package com.z2rs.game

import android.content.Intent
import android.content.res.Configuration
import android.net.Uri
import android.os.Bundle
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.text.format.Formatter
import android.util.Log
import android.view.View
import android.widget.RadioGroup
import android.widget.TextView
import androidx.activity.result.contract.ActivityResultContracts
import androidx.appcompat.app.AppCompatActivity
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.updatePadding
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.lifecycleScope
import androidx.lifecycle.repeatOnLifecycle
import com.google.android.material.button.MaterialButton
import com.google.android.material.materialswitch.MaterialSwitch
import com.google.android.material.progressindicator.LinearProgressIndicator
import com.google.android.material.slider.Slider
import dalvik.system.BaseDexClassLoader
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import java.io.File
import java.io.IOException

/**
 * Entry screen: pick the ROM (Storage Access Framework, copied to
 * filesDir/rom.nes), optionally import an HD graphics pack (a .zip or a
 * folder, unpacked to filesDir/hd_pack by [HdPackImporter]), change settings,
 * and Play. Play writes filesDir/launch_args.json and starts [GameActivity]
 * in its own process.
 *
 * Only a size sanity check happens here; the Rust side verifies the ROM's
 * hashes when the game starts and reports a wrong dump there.
 */
class LauncherActivity : AppCompatActivity() {

    private lateinit var settings: Settings
    private lateinit var status: TextView
    private lateinit var chooseRom: MaterialButton
    private lateinit var play: MaterialButton
    private var copying = false

    private lateinit var hdStore: HdPackStore
    private lateinit var hdStatus: TextView
    private lateinit var hdDetail: TextView
    private lateinit var hdProgress: LinearProgressIndicator
    private lateinit var hdImportZip: MaterialButton
    private lateinit var hdImportFolder: MaterialButton
    private lateinit var hdCancel: MaterialButton
    private lateinit var hdRemove: MaterialButton

    private val romFile get() = File(filesDir, ROM_FILE)

    private val pickRom = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) copyRom(uri)
    }

    private val pickPackZip = registerForActivityResult(ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) HdPackImporter.importZip(this, uri, displayName(uri))
    }

    private val pickPackFolder = registerForActivityResult(ActivityResultContracts.OpenDocumentTree()) { uri ->
        if (uri != null) HdPackImporter.importFolder(this, uri, treeName(uri))
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Tapping the home-screen icon while a game is running can start a
        // second launcher on top of it (when the task's root intent differs
        // from the icon's). Close that copy so the running game comes back.
        if (!isTaskRoot && intent?.action == Intent.ACTION_MAIN &&
            intent.hasCategory(Intent.CATEGORY_LAUNCHER)
        ) {
            finish()
            return
        }
        setContentView(R.layout.activity_launcher)
        settings = Settings(this)

        // targetSdk 35 draws edge to edge: dark bar icons on the light theme, and
        // keep the content out of the bars and cutout.
        val night = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
            Configuration.UI_MODE_NIGHT_YES
        WindowCompat.getInsetsController(window, window.decorView).apply {
            isAppearanceLightStatusBars = !night
            isAppearanceLightNavigationBars = !night
        }
        val content = findViewById<android.view.View>(R.id.content)
        val base = content.paddingTop
        val baseBottom = content.paddingBottom
        val baseStart = content.paddingLeft
        val baseEnd = content.paddingRight
        ViewCompat.setOnApplyWindowInsetsListener(findViewById(R.id.scroll)) { _, insets ->
            val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
            content.updatePadding(
                left = baseStart + bars.left,
                top = base + bars.top,
                right = baseEnd + bars.right,
                bottom = baseBottom + bars.bottom,
            )
            insets
        }

        status = findViewById(R.id.rom_status)
        chooseRom = findViewById(R.id.choose_rom)
        play = findViewById(R.id.play)

        chooseRom.setOnClickListener {
            // Any file type: providers label .nes files inconsistently.
            pickRom.launch(arrayOf("*/*"))
        }
        play.setOnClickListener { launchGame() }

        bindSwitch(R.id.pref_touch_enabled, settings.touchEnabled) {
            settings.touchEnabled = it
            refreshDependents()
        }
        bindSwitch(R.id.pref_touch_hide_controller, settings.hideTouchWithController) { settings.hideTouchWithController = it }
        bindSwitch(R.id.pref_haptics, settings.haptics) { settings.haptics = it }
        bindSwitch(R.id.pref_left_handed, settings.leftHanded) { settings.leftHanded = it }
        bindSwitch(R.id.pref_high_contrast, settings.highContrast) { settings.highContrast = it }
        bindSwitch(R.id.pref_keep_screen_on, settings.keepScreenOn) { settings.keepScreenOn = it }
        bindSwitch(R.id.pref_coop_local, settings.coopLocal) { settings.coopLocal = it }

        bindRadio(
            R.id.pad_size,
            listOf(R.id.pad_size_small to 0, R.id.pad_size_medium to 1, R.id.pad_size_large to 2),
            settings.padSize,
        ) { settings.padSize = it }
        bindRadio(
            R.id.widescreen,
            listOf(
                R.id.widescreen_off to "off",
                R.id.widescreen_16_10 to "16:10",
                R.id.widescreen_16_9 to "16:9",
                R.id.widescreen_21_9 to "21:9",
            ),
            settings.widescreen,
        ) { settings.widescreen = it }
        bindRadio(
            R.id.scale_mode,
            listOf(R.id.scale_fit to "fit", R.id.scale_integer to "integer"),
            settings.scaleMode,
        ) { settings.scaleMode = it }

        val opacity = findViewById<Slider>(R.id.pad_opacity)
        opacity.value = snapOpacity(settings.padOpacity).toFloat()
        opacity.contentDescription = getString(R.string.pref_pad_opacity)
        opacity.setLabelFormatter { v -> getString(R.string.pref_pad_opacity_value, v.toInt()) }
        opacity.addOnChangeListener { _, v, fromUser -> if (fromUser) settings.padOpacity = v.toInt() }

        // Radio groups announce their heading with each choice.
        labelGroup(R.id.pad_size, R.id.pad_size_label)
        labelGroup(R.id.widescreen, R.id.widescreen_label)
        labelGroup(R.id.scale_mode, R.id.scale_label)

        setUpHdPack()

        refreshDependents()
        refreshRomState()
    }

    override fun onResume() {
        super.onResume()
        refreshRomState()
        showLastGameError()
    }

    /** The game process leaves a one-line reason behind when it could not start. */
    private fun showLastGameError() {
        val f = File(filesDir, LAST_ERROR_FILE)
        if (!f.isFile) return
        val msg = try {
            f.readText().trim()
        } catch (e: IOException) {
            ""
        }
        f.delete()
        if (msg.isNotEmpty()) status.text = getString(R.string.last_game_error, msg)
    }

    private fun snapOpacity(v: Int): Int = ((v + 5) / 10 * 10).coerceIn(Settings.MIN_OPACITY, 100)

    private fun bindSwitch(id: Int, value: Boolean, onChange: (Boolean) -> Unit) {
        findViewById<MaterialSwitch>(id).apply {
            isChecked = value
            setOnCheckedChangeListener { _, checked -> onChange(checked) }
        }
    }

    private fun <T> bindRadio(groupId: Int, options: List<Pair<Int, T>>, current: T, onChange: (T) -> Unit) {
        val group = findViewById<RadioGroup>(groupId)
        options.firstOrNull { it.second == current }?.let { group.check(it.first) }
        group.setOnCheckedChangeListener { _, checkedId ->
            options.firstOrNull { it.first == checkedId }?.let { onChange(it.second) }
        }
    }

    private fun labelGroup(groupId: Int, labelId: Int) {
        val label = findViewById<TextView>(labelId)
        findViewById<RadioGroup>(groupId).contentDescription = label.text
    }

    /** Pad options only matter while the on-screen pad is enabled. */
    private fun refreshDependents() {
        val on = settings.touchEnabled
        for (id in listOf(
            R.id.pref_touch_hide_controller, R.id.pad_size_small, R.id.pad_size_medium,
            R.id.pad_size_large, R.id.pad_opacity, R.id.pref_haptics, R.id.pref_left_handed,
            R.id.pref_high_contrast,
        )) {
            findViewById<android.view.View>(id).isEnabled = on
        }
    }

    // --- ROM -----------------------------------------------------------------

    private fun refreshRomState() {
        val f = romFile
        val have = f.isFile && LaunchArgs.plausibleRomSize(f.length())
        if (!copying) {
            status.text = if (have) {
                val size = Formatter.formatShortFileSize(this, f.length())
                settings.romName?.let { getString(R.string.rom_ready, it, size) }
                    ?: getString(R.string.rom_ready_unnamed, size)
            } else {
                getString(R.string.rom_none)
            }
        }
        chooseRom.setText(if (have) R.string.rom_replace else R.string.rom_choose)
        chooseRom.contentDescription = getString(if (have) R.string.rom_replace_desc else R.string.rom_choose_desc)
        chooseRom.isEnabled = !copying
        refreshPlay()
    }

    private fun refreshPlay() {
        val f = romFile
        val have = f.isFile && LaunchArgs.plausibleRomSize(f.length())
        // The pack folder is swapped at the end of an import; don't start the game mid-swap.
        val importing = HdPackImporter.running
        play.isEnabled = have && !copying && !importing
        play.contentDescription = when {
            play.isEnabled -> null
            importing && have -> getString(R.string.play_desc_importing)
            else -> getString(R.string.play_desc_disabled)
        }
    }

    private fun copyRom(uri: Uri) {
        copying = true
        status.setText(R.string.rom_copying)
        refreshRomState()
        lifecycleScope.launch {
            val name = displayName(uri)
            val result = withContext(Dispatchers.IO) { copyToRomFile(uri) }
            copying = false
            when (result) {
                is CopyResult.Ok -> {
                    settings.romName = name
                    refreshRomState()
                    status.text = getString(R.string.rom_copied, name ?: Formatter.formatShortFileSize(this@LauncherActivity, result.bytes))
                }
                is CopyResult.BadSize -> {
                    refreshRomState()
                    status.text = getString(R.string.rom_error_size, Formatter.formatShortFileSize(this@LauncherActivity, result.bytes))
                }
                is CopyResult.Failed -> {
                    refreshRomState()
                    status.text = getString(R.string.rom_error_read, result.message)
                }
            }
        }
    }

    private sealed class CopyResult {
        data class Ok(val bytes: Long) : CopyResult()
        data class BadSize(val bytes: Long) : CopyResult()
        data class Failed(val message: String) : CopyResult()
    }

    /** Copies through a temp file, so a failed or rejected pick leaves the old ROM alone. */
    private fun copyToRomFile(uri: Uri): CopyResult {
        val tmp = File(filesDir, "$ROM_FILE.tmp")
        return try {
            val input = contentResolver.openInputStream(uri) ?: return CopyResult.Failed("no data")
            var total = 0L
            input.use { ins ->
                tmp.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = ins.read(buf)
                        if (n < 0) break
                        total += n
                        if (total > MAX_READ) break // far too big for a NES ROM; stop reading
                        out.write(buf, 0, n)
                    }
                }
            }
            if (!LaunchArgs.plausibleRomSize(total)) {
                tmp.delete()
                return CopyResult.BadSize(total)
            }
            if (!tmp.renameTo(romFile)) {
                tmp.delete()
                return CopyResult.Failed("could not store the copy")
            }
            CopyResult.Ok(total)
        } catch (e: IOException) {
            tmp.delete()
            CopyResult.Failed(e.message ?: e.javaClass.simpleName)
        } catch (e: SecurityException) {
            tmp.delete()
            CopyResult.Failed(e.message ?: e.javaClass.simpleName)
        }
    }

    private fun displayName(uri: Uri): String? = try {
        contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
            if (c.moveToFirst()) c.getString(0) else null
        }
    } catch (e: Exception) {
        null
    }

    // --- HD graphics pack ----------------------------------------------------

    private fun setUpHdPack() {
        hdStore = HdPackStore(filesDir)
        hdStatus = findViewById(R.id.hd_pack_status)
        hdDetail = findViewById(R.id.hd_pack_detail)
        hdProgress = findViewById(R.id.hd_pack_progress)
        hdImportZip = findViewById(R.id.hd_pack_import_zip)
        hdImportFolder = findViewById(R.id.hd_pack_import_folder)
        hdCancel = findViewById(R.id.hd_pack_cancel)
        hdRemove = findViewById(R.id.hd_pack_remove)

        if (!HdPackImporter.running) hdStore.cleanUpLeftovers()

        // Any file type, as for the ROM: providers label .zip files inconsistently.
        hdImportZip.setOnClickListener { pickPackZip.launch(arrayOf("*/*")) }
        hdImportFolder.setOnClickListener { pickPackFolder.launch(null) }
        hdCancel.setOnClickListener { HdPackImporter.cancel() }
        hdRemove.setOnClickListener {
            if (HdPackImporter.running) return@setOnClickListener
            hdStore.remove()
            settings.clearHdPack()
            HdPackImporter.acknowledge()
            renderHdPack(HdPackImporter.State.Idle)
            hdStatus.setText(R.string.hd_pack_removed)
        }

        lifecycleScope.launch {
            repeatOnLifecycle(Lifecycle.State.STARTED) {
                HdPackImporter.state.collect { renderHdPack(it) }
            }
        }
    }

    private fun renderHdPack(state: HdPackImporter.State) {
        val busy = state is HdPackImporter.State.Copying || state is HdPackImporter.State.Checking
        val root = settings.hdPackRoot
        val installed = root != null && hdStore.isInstalled(root)

        hdProgress.visibility = if (busy) View.VISIBLE else View.GONE
        hdCancel.visibility = if (state is HdPackImporter.State.Copying) View.VISIBLE else View.GONE
        hdDetail.visibility = if (state is HdPackImporter.State.Copying) View.VISIBLE else View.GONE
        hdRemove.visibility = if (installed && !busy) View.VISIBLE else View.GONE
        hdImportZip.isEnabled = !busy
        hdImportFolder.isEnabled = !busy
        hdImportZip.setText(if (installed) R.string.hd_pack_replace_zip else R.string.hd_pack_import_zip)
        hdImportZip.contentDescription =
            getString(if (installed) R.string.hd_pack_replace_zip_desc else R.string.hd_pack_import_zip_desc)
        hdImportFolder.setText(if (installed) R.string.hd_pack_replace_folder else R.string.hd_pack_import_folder)
        hdImportFolder.contentDescription =
            getString(if (installed) R.string.hd_pack_replace_folder_desc else R.string.hd_pack_import_folder_desc)

        when (state) {
            is HdPackImporter.State.Idle -> hdStatus.text = installedText(root, installed)
            is HdPackImporter.State.Copying -> {
                // Set once, so the live region announces it once.
                val text = getString(R.string.hd_pack_copying)
                if (hdStatus.text.toString() != text) hdStatus.text = text
                hdDetail.text = resources.getQuantityString(
                    R.plurals.hd_pack_copying_detail,
                    state.files,
                    state.files,
                    Formatter.formatShortFileSize(this, state.bytes),
                )
            }
            is HdPackImporter.State.Checking -> hdStatus.setText(R.string.hd_pack_checking)
            is HdPackImporter.State.Done.Installed -> hdStatus.text = getString(R.string.hd_pack_installed, state.name)
            is HdPackImporter.State.Done.Invalid -> hdStatus.text = getString(R.string.hd_pack_invalid, state.message)
            is HdPackImporter.State.Done.Failed -> hdStatus.text = getString(R.string.hd_pack_error, failureText(state))
            is HdPackImporter.State.Done.Cancelled -> hdStatus.setText(R.string.hd_pack_cancelled)
        }
        refreshPlay()
    }

    private fun installedText(root: String?, installed: Boolean): String = when {
        root == null -> getString(R.string.hd_pack_none)
        !installed -> getString(R.string.hd_pack_missing)
        settings.hdPackTiles >= 0 -> resources.getQuantityString(
            R.plurals.hd_pack_ready,
            settings.hdPackTiles,
            settings.hdPackName ?: getString(R.string.hd_pack_default_name),
            settings.hdPackTiles,
        )
        else -> getString(R.string.hd_pack_ready_unchecked, settings.hdPackName ?: getString(R.string.hd_pack_default_name))
    }

    private fun failureText(f: HdPackImporter.State.Done.Failed): String {
        val limits = HdPackImporter.limits
        val size = { b: Long -> Formatter.formatShortFileSize(this, b) }
        return when (f.kind) {
            HdPackImport.Kind.UNSAFE_PATH -> getString(R.string.hd_pack_err_unsafe, f.detail.take(120))
            HdPackImport.Kind.TOO_MANY_FILES ->
                resources.getQuantityString(R.plurals.hd_pack_err_too_many, limits.maxFiles, limits.maxFiles)
            HdPackImport.Kind.FILE_TOO_BIG ->
                getString(R.string.hd_pack_err_file_too_big, f.detail.take(120), size(limits.maxFileBytes))
            HdPackImport.Kind.TOTAL_TOO_BIG -> getString(R.string.hd_pack_err_too_big, size(limits.maxTotalBytes))
            HdPackImport.Kind.NOT_A_ZIP -> getString(R.string.hd_pack_err_not_zip)
            HdPackImport.Kind.NO_MANIFEST -> getString(R.string.hd_pack_err_no_manifest)
            HdPackImport.Kind.NO_SPACE -> getString(R.string.hd_pack_err_space)
            null -> f.detail
        }
    }

    /** The picked folder's own name, for the status line when the pack cannot be checked. */
    private fun treeName(tree: Uri): String? = try {
        displayName(DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree)))
    } catch (e: Exception) {
        null
    }

    /** The installed pack's folder for `--hd-pack`, or null for none. */
    private fun hdPackPath(): String? {
        val root = settings.hdPackRoot ?: return null
        return if (hdStore.isInstalled(root)) hdStore.packDir(root).absolutePath else null
    }

    // --- Play ----------------------------------------------------------------

    private fun nativeLibraryPresent(): Boolean = try {
        (classLoader as? BaseDexClassLoader)?.findLibrary(NATIVE_LIB) != null
    } catch (e: Exception) {
        true // cannot tell; let GameActivity try
    }

    private fun launchGame() {
        if (!romFile.isFile) {
            refreshRomState()
            return
        }
        if (!nativeLibraryPresent()) {
            status.setText(R.string.no_native_lib)
            return
        }
        if (HdPackImporter.running) return
        try {
            val args = LaunchArgs.build(
                romPath = romFile.absolutePath,
                widescreen = settings.widescreen,
                scaleMode = settings.scaleMode,
                coopLocal = settings.coopLocal,
                hdPack = hdPackPath(),
            )
            val tmp = File(filesDir, LaunchArgs.FILE_NAME + ".tmp")
            tmp.writeText(LaunchArgs.toJson(args))
            if (!tmp.renameTo(File(filesDir, LaunchArgs.FILE_NAME))) throw IOException("rename failed")
            HdPackImporter.acknowledge()
            startActivity(Intent(this, GameActivity::class.java))
        } catch (e: Exception) {
            Log.e(TAG, "launch failed", e)
            status.text = getString(R.string.launch_error, e.message ?: e.javaClass.simpleName)
        }
    }

    private companion object {
        const val TAG = "z2rs"
        const val ROM_FILE = "rom.nes"
        const val NATIVE_LIB = "z2rs_android"
        const val MAX_READ = 8L * 1024 * 1024
        const val LAST_ERROR_FILE = "last_game_error.txt"
    }
}
