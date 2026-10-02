package com.z2rs.game

import java.io.File
import java.io.IOException
import java.io.InputStream
import java.util.zip.ZipException
import java.util.zip.ZipInputStream

/**
 * HD pack import, the platform-free half: entry path checks (no zip-slip),
 * size and file-count limits, zip extraction, finding the folder that holds
 * pack.json, the staging/commit swap in app storage, and parsing the native
 * check's reply. Only java.io and java.util.zip, so it is unit-tested on the
 * JVM; [HdPackImporter] adds the Storage Access Framework side.
 *
 * Whether the files are a valid pack is decided by the game's own loader,
 * over JNI ([Native.checkHdPack]), not here.
 */
object HdPackImport {
    const val MANIFEST = "pack.json"

    data class Limits(
        val maxFiles: Int = 20_000,
        /** Any one file; HD sheets are at most 4096 px square PNGs. */
        val maxFileBytes: Long = 64L * 1024 * 1024,
        /** Everything unpacked. */
        val maxTotalBytes: Long = 1024L * 1024 * 1024,
        /** Folder levels below the picked zip or folder. */
        val maxDepth: Int = 16,
        /** Storage to leave free on the device while importing. */
        val reserveBytes: Long = 64L * 1024 * 1024,
    )

    enum class Kind { UNSAFE_PATH, TOO_MANY_FILES, FILE_TOO_BIG, TOTAL_TOO_BIG, NOT_A_ZIP, NO_MANIFEST, NO_SPACE }

    /** An import refused for a reason the launcher explains; [detail] is a path or a number. */
    class ImportException(val kind: Kind, val detail: String = "") : IOException("$kind $detail")

    sealed class EntryPath {
        /** Store the file at this normalised relative path. */
        data class Keep(val path: String) : EntryPath()

        /** Not part of a pack: directory entries, `__MACOSX`, dot files, Thumbs.db. */
        object Skip : EntryPath()

        /** Would land outside the pack folder or cannot be stored; refuse the whole import. */
        data class Unsafe(val reason: String) : EntryPath()
    }

    /**
     * Check one zip entry name (or a folder-relative path) before anything is
     * written. `\` counts as a separator, `.` and empty segments are dropped,
     * and absolute, drive-qualified and `..` paths are refused outright, as
     * the pack loader's own path rule does for pack.json entries.
     */
    fun checkEntryPath(raw: String, maxDepth: Int = Limits().maxDepth): EntryPath {
        if (raw.indexOf('\u0000') >= 0) return EntryPath.Unsafe("NUL in name")
        val s = raw.replace('\\', '/')
        if (s.startsWith("/")) return EntryPath.Unsafe("absolute path")
        if (s.length >= 2 && s[1] == ':' && s[0].isLetter()) return EntryPath.Unsafe("drive letter")
        val parts = s.split('/').filter { it.isNotEmpty() && it != "." }
        if (parts.any { it == ".." }) return EntryPath.Unsafe("\"..\" in path")
        // Directory entries ("sheets/"): folders are created for the files in them.
        if (parts.isEmpty() || s.endsWith("/")) return EntryPath.Skip
        if (parts.any { it.startsWith(".") || it == "__MACOSX" || it.equals("Thumbs.db", true) || it.equals("desktop.ini", true) }) {
            return EntryPath.Skip
        }
        if (parts.size > maxDepth) return EntryPath.Unsafe("nested more than $maxDepth folders deep")
        if (parts.any { it.toByteArray(Charsets.UTF_8).size > 255 }) return EntryPath.Unsafe("name too long")
        return EntryPath.Keep(parts.joinToString("/"))
    }

    /**
     * The folder (relative, `""` or ending in `/`) holding the shallowest
     * pack.json among [paths], ties broken by name: the same rule as the
     * loader's HdPack::from_files. Zips often wrap the pack in one extra
     * folder; this finds it either way. Null when there is none.
     */
    fun findPackRoot(paths: Collection<String>): String? =
        paths.filter { it == MANIFEST || it.endsWith("/$MANIFEST") }
            .minWithOrNull(compareBy<String>({ p -> p.count { it == '/' } }, { it }))
            ?.removeSuffix(MANIFEST)

    /**
     * Counts files and bytes against [limits] while copying them under a
     * destination folder. [onProgress] runs after every chunk; it may throw
     * (a CancellationException) to stop the import. [freeBytes] reports the
     * free space on the destination's volume.
     */
    class Budget(
        private val limits: Limits,
        private val freeBytes: () -> Long = { Long.MAX_VALUE },
        private val onProgress: (files: Int, bytes: Long) -> Unit = { _, _ -> },
    ) {
        var files = 0
            private set
        var bytes = 0L
            private set

        /** Copy [input] to [rel] under [root]. [rel] must come from [checkEntryPath]. */
        fun copyFile(root: File, rel: String, input: InputStream) {
            if (files >= limits.maxFiles) throw ImportException(Kind.TOO_MANY_FILES, limits.maxFiles.toString())
            if (freeBytes() < limits.reserveBytes) throw ImportException(Kind.NO_SPACE)
            val target = resolveInside(root, rel)
            target.parentFile?.let { if (!it.isDirectory && !it.mkdirs()) throw IOException("could not create ${it.name}") }
            var written = 0L
            target.outputStream().use { out ->
                val buf = ByteArray(64 * 1024)
                while (true) {
                    val n = input.read(buf)
                    if (n < 0) break
                    written += n
                    bytes += n
                    // Counted as written, not as the zip declares: a zip bomb stops here.
                    if (written > limits.maxFileBytes) throw ImportException(Kind.FILE_TOO_BIG, rel)
                    if (bytes > limits.maxTotalBytes) throw ImportException(Kind.TOTAL_TOO_BIG)
                    out.write(buf, 0, n)
                    onProgress(files, bytes)
                }
            }
            files++
            onProgress(files, bytes)
        }
    }

    /** [rel] under [root], refusing anything that resolves outside it (second line of defence). */
    fun resolveInside(root: File, rel: String): File {
        val base = root.canonicalFile
        val target = File(base, rel).canonicalFile
        if (!target.path.startsWith(base.path + File.separator)) throw ImportException(Kind.UNSAFE_PATH, rel)
        return target
    }

    /**
     * Unpack the zip in [input] under [dest] (which should be empty). Returns
     * the relative paths written. Refuses the whole zip on the first unsafe
     * path or exceeded limit; the caller deletes [dest] then.
     */
    fun extractZip(input: InputStream, dest: File, limits: Limits, budget: Budget): List<String> {
        val kept = mutableListOf<String>()
        var entries = 0
        try {
            ZipInputStream(input.buffered()).use { zip ->
                while (true) {
                    val e = zip.nextEntry ?: break
                    entries++
                    when (val p = checkEntryPath(e.name, limits.maxDepth)) {
                        is EntryPath.Unsafe -> throw ImportException(Kind.UNSAFE_PATH, e.name)
                        is EntryPath.Skip -> Unit
                        is EntryPath.Keep -> if (!e.isDirectory) {
                            budget.copyFile(dest, p.path, zip)
                            kept += p.path
                        }
                    }
                }
            }
        } catch (e: ZipException) {
            throw zipFailure(e.message)
        } catch (e: IllegalArgumentException) {
            // Entry names that are not valid in the zip's charset.
            throw ImportException(Kind.NOT_A_ZIP, e.message ?: "")
        }
        // ZipInputStream reads a file that is not a zip as zero entries.
        if (entries == 0) throw ImportException(Kind.NOT_A_ZIP)
        return kept
    }

    /**
     * A ZipException as an import failure. Android 14+ (targetSdk 34+) checks
     * entry names itself before we see them (ZipPathValidator) and throws
     * "Invalid zip entry path: <name>" for a ".." or absolute one: that is a
     * zip-slip refusal, not an unreadable zip.
     */
    fun zipFailure(message: String?): ImportException {
        val m = message.orEmpty()
        val marker = "zip entry path:"
        val at = m.indexOf(marker, ignoreCase = true)
        return if (at >= 0) {
            ImportException(Kind.UNSAFE_PATH, m.substring(at + marker.length).trim())
        } else {
            ImportException(Kind.NOT_A_ZIP, m)
        }
    }

    /** Result of [Native.checkHdPack]. */
    sealed class PackCheck {
        data class Ok(val name: String, val scale: Int, val tiles: Int, val layers: Int) : PackCheck()
        data class Bad(val message: String) : PackCheck()

        /** The game library could not be loaded here; the game checks the pack when it starts. */
        object Unavailable : PackCheck()
    }

    /** Parse `ok\t<name>\t<scale>\t<tiles>\t<layers>` / `error\t<message>` (z2_native::hd_check::check_reply). */
    fun parseCheckReply(reply: String?): PackCheck {
        if (reply == null) return PackCheck.Unavailable
        val f = reply.split('\t')
        return when {
            f[0] == "ok" && f.size == 5 -> {
                val n = f.drop(2).map { it.toIntOrNull() }
                if (n.any { it == null }) PackCheck.Bad("unexpected reply: $reply")
                else PackCheck.Ok(f[1], n[0]!!, n[1]!!, n[2]!!)
            }
            f[0] == "error" && f.size >= 2 -> PackCheck.Bad(f.drop(1).joinToString(" "))
            else -> PackCheck.Bad("unexpected reply: $reply")
        }
    }
}

/**
 * Where an imported pack lives: `filesDir/hd_pack/` (the pack.json may sit in
 * a subfolder, kept as the "root" prefix). An import unpacks into
 * `hd_pack.staging/` and only replaces the installed pack once it checked
 * out, so a failed or cancelled import leaves the previous pack alone.
 */
class HdPackStore(filesDir: File) {
    val installDir = File(filesDir, INSTALL)
    val stagingDir = File(filesDir, STAGING)
    private val oldDir = File(filesDir, OLD)

    /** An empty staging folder. */
    fun freshStaging(): File {
        stagingDir.deleteRecursively()
        if (!stagingDir.mkdirs()) throw IOException("could not create ${stagingDir.name}")
        return stagingDir
    }

    fun discardStaging() {
        stagingDir.deleteRecursively()
    }

    /** Make the staging folder the installed pack. */
    fun commit() {
        oldDir.deleteRecursively()
        if (installDir.exists() && !installDir.renameTo(oldDir)) throw IOException("could not replace the old pack")
        if (!stagingDir.renameTo(installDir)) {
            oldDir.renameTo(installDir)
            throw IOException("could not store the pack")
        }
        oldDir.deleteRecursively()
    }

    fun remove() {
        installDir.deleteRecursively()
        oldDir.deleteRecursively()
    }

    /** Leftovers of an import the process did not live to finish. */
    fun cleanUpLeftovers() {
        stagingDir.deleteRecursively()
        oldDir.deleteRecursively()
    }

    /** The installed pack's folder for a root prefix from [HdPackImport.findPackRoot]. */
    fun packDir(root: String): File = if (root.isEmpty()) installDir else File(installDir, root.trimEnd('/'))

    fun isInstalled(root: String): Boolean = File(packDir(root), HdPackImport.MANIFEST).isFile

    companion object {
        const val INSTALL = "hd_pack"
        const val STAGING = "hd_pack.staging"
        const val OLD = "hd_pack.old"
    }
}
