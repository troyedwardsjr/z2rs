package com.z2rs.game

import android.content.ContentResolver
import android.content.Context
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import java.io.File
import java.io.IOException

/**
 * Imports an HD pack picked through the Storage Access Framework (a .zip with
 * ACTION_OPEN_DOCUMENT, or an unzipped folder with ACTION_OPEN_DOCUMENT_TREE)
 * into app-private storage, then checks it with the game's loader.
 *
 * The import runs in a process-wide scope, not the activity's, so rotating
 * the launcher or switching apps does not abort it; the launcher just shows
 * [state]. One import at a time.
 */
object HdPackImporter {
    private const val TAG = "z2rs"

    sealed class State {
        object Idle : State()
        data class Copying(val files: Int, val bytes: Long) : State()
        object Checking : State()
        sealed class Done : State() {
            /** [tiles] is null when the game library could not check the pack here. */
            data class Installed(val name: String, val tiles: Int?) : Done()
            data class Failed(val kind: HdPackImport.Kind?, val detail: String) : Done()

            /** The check ran and the loader refused the pack. */
            data class Invalid(val message: String) : Done()
            object Cancelled : Done()
        }
    }

    val limits = HdPackImport.Limits()

    private val _state = MutableStateFlow<State>(State.Idle)
    val state: StateFlow<State> = _state

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    @Volatile private var job: Job? = null

    /** An import is copying or checking (its result is not in yet). Main thread. */
    val running: Boolean
        get() = _state.value.let { it is State.Copying || it is State.Checking }

    /** Import a .zip document. */
    fun importZip(context: Context, uri: Uri, displayName: String?) =
        start(context, displayName) { cr, dest, budget ->
            val input = cr.openInputStream(uri) ?: throw IOException("no data")
            input.use { HdPackImport.extractZip(it, dest, limits, budget) }
        }

    /** Import every file under a picked folder (a document tree). */
    fun importFolder(context: Context, treeUri: Uri, displayName: String?) =
        start(context, displayName) { cr, dest, budget -> copyTree(cr, treeUri, dest, budget) }

    fun cancel() {
        job?.cancel()
    }

    /** The launcher showed a finished import's result. */
    fun acknowledge() {
        if (_state.value is State.Done) _state.value = State.Idle
    }

    private fun start(
        context: Context,
        displayName: String?,
        copy: (ContentResolver, File, HdPackImport.Budget) -> List<String>,
    ) {
        if (running) return
        val app = context.applicationContext
        val store = HdPackStore(app.filesDir)
        _state.value = State.Copying(0, 0)
        job = scope.launch {
            val me = coroutineContext.job
            var lastReport = 0L
            val budget = HdPackImport.Budget(
                limits,
                freeBytes = { app.filesDir.usableSpace },
                onProgress = { files, bytes ->
                    me.ensureActive()
                    // A few updates a second are plenty for the status line.
                    val now = System.nanoTime()
                    if (now - lastReport > 200_000_000L) {
                        lastReport = now
                        _state.value = State.Copying(files, bytes)
                    }
                },
            )
            _state.value = try {
                val dest = store.freshStaging()
                val kept = copy(app.contentResolver, dest, budget)
                me.ensureActive()
                val root = HdPackImport.findPackRoot(kept)
                    ?: throw HdPackImport.ImportException(HdPackImport.Kind.NO_MANIFEST)
                _state.value = State.Checking
                val check = HdPackImport.parseCheckReply(Native.checkHdPack(store.stagingDir.resolve(root).absolutePath))
                me.ensureActive()
                when (check) {
                    is HdPackImport.PackCheck.Bad -> {
                        store.discardStaging()
                        State.Done.Invalid(check.message)
                    }
                    is HdPackImport.PackCheck.Ok, HdPackImport.PackCheck.Unavailable -> {
                        store.commit()
                        val settings = Settings(app)
                        val ok = check as? HdPackImport.PackCheck.Ok
                        val name = ok?.name?.takeIf { it.isNotBlank() }
                            ?: displayName
                            ?: root.trimEnd('/').ifEmpty { app.getString(R.string.hd_pack_default_name) }
                        settings.setHdPack(root, name, ok?.tiles ?: -1)
                        State.Done.Installed(name, ok?.tiles)
                    }
                }
            } catch (e: CancellationException) {
                store.discardStaging()
                State.Done.Cancelled
            } catch (e: HdPackImport.ImportException) {
                store.discardStaging()
                Log.i(TAG, "HD pack import refused: ${e.kind} ${e.detail}")
                State.Done.Failed(e.kind, e.detail)
            } catch (e: IOException) {
                Log.w(TAG, "HD pack import failed", e)
                store.discardStaging()
                State.Done.Failed(null, e.message ?: e.javaClass.simpleName)
            } catch (e: SecurityException) {
                store.discardStaging()
                State.Done.Failed(null, e.message ?: e.javaClass.simpleName)
            } catch (e: IllegalArgumentException) {
                // DocumentsContract on a URI the provider does not understand.
                store.discardStaging()
                State.Done.Failed(null, e.message ?: e.javaClass.simpleName)
            }
        }
    }

    /** Copy a document tree, depth first, through the same path checks and limits as a zip. */
    private fun copyTree(cr: ContentResolver, treeUri: Uri, dest: File, budget: HdPackImport.Budget): List<String> {
        val kept = mutableListOf<String>()
        val cols = arrayOf(
            DocumentsContract.Document.COLUMN_DOCUMENT_ID,
            DocumentsContract.Document.COLUMN_DISPLAY_NAME,
            DocumentsContract.Document.COLUMN_MIME_TYPE,
        )

        fun walk(docId: String, prefix: String) {
            val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, docId)
            val cursor = cr.query(children, cols, null, null, null) ?: throw IOException("could not list the folder")
            cursor.use { c ->
                while (c.moveToNext()) {
                    val id = c.getString(0) ?: continue
                    val name = c.getString(1) ?: continue
                    val isDir = c.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR
                    val rel = if (prefix.isEmpty()) name else "$prefix/$name"
                    when (val p = HdPackImport.checkEntryPath(rel, limits.maxDepth)) {
                        is HdPackImport.EntryPath.Unsafe -> throw HdPackImport.ImportException(HdPackImport.Kind.UNSAFE_PATH, rel)
                        is HdPackImport.EntryPath.Skip -> Unit
                        is HdPackImport.EntryPath.Keep -> if (isDir) {
                            walk(id, p.path)
                        } else {
                            val uri = DocumentsContract.buildDocumentUriUsingTree(treeUri, id)
                            val input = cr.openInputStream(uri) ?: throw IOException("could not open $rel")
                            input.use { budget.copyFile(dest, p.path, it) }
                            kept += p.path
                        }
                    }
                }
            }
        }

        walk(DocumentsContract.getTreeDocumentId(treeUri), "")
        return kept
    }
}
