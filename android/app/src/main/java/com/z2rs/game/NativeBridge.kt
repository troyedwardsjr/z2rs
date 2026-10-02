package com.z2rs.game

import android.util.Log

/**
 * JNI surface implemented by libz2rs_android.so (crates/z2-android). The
 * library is loaded by GameActivity itself (System.loadLibrary of the
 * android.app.lib_name meta-data), so these resolve as
 * Java_com_z2rs_game_NativeBridge_<name>(JNIEnv*, jclass, ...).
 *
 * Do not call these directly: go through [Native], which survives a missing
 * library or symbol.
 */
object NativeBridge {
    @JvmStatic external fun setTouchPad(mask: Int)
    @JvmStatic external fun setHardwarePad(player: Int, mask: Int)
    @JvmStatic external fun setPaused(paused: Boolean)
    @JvmStatic external fun requestSaveState(slot: Int)
    @JvmStatic external fun requestLoadState(slot: Int)

    /** Load the HD pack in `dir` with the game's loader (blocking); see [Native.checkHdPack]. */
    @JvmStatic external fun checkHdPack(dir: String): String?
}

/**
 * Guarded calls into [NativeBridge]. An UnsatisfiedLinkError (library not
 * loaded, or a symbol the Rust side does not export) is logged once and the
 * call is dropped, so the UI can never crash on it.
 */
object Native {
    private const val TAG = "z2rs"
    private const val LIB = "z2rs_android"
    @Volatile private var warned = false

    fun setTouchPad(mask: Int) = guard { NativeBridge.setTouchPad(mask and 0xFF) }
    fun setHardwarePad(player: Int, mask: Int) = guard { NativeBridge.setHardwarePad(player, mask and 0xFF) }
    fun setPaused(paused: Boolean) = guard { NativeBridge.setPaused(paused) }
    fun requestSaveState(slot: Int) = guard { NativeBridge.requestSaveState(slot) }
    fun requestLoadState(slot: Int) = guard { NativeBridge.requestLoadState(slot) }

    /**
     * Check an imported HD pack with the game's own loader, from the
     * launcher's process (which does not load the game library on its own,
     * so this loads it first). Blocking; call it off the main thread.
     * Returns z2_native::hd_check's reply line, or null when the library or
     * the symbol is unavailable ([HdPackImport.parseCheckReply] reads both).
     */
    fun checkHdPack(dir: String): String? {
        if (!libraryLoaded) return null
        return try {
            NativeBridge.checkHdPack(dir)
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "checkHdPack unavailable: ${e.message}")
            null
        }
    }

    private val libraryLoaded: Boolean by lazy {
        try {
            System.loadLibrary(LIB)
            true
        } catch (e: UnsatisfiedLinkError) {
            Log.w(TAG, "could not load lib$LIB.so: ${e.message}")
            false
        } catch (e: SecurityException) {
            Log.w(TAG, "could not load lib$LIB.so: ${e.message}")
            false
        }
    }

    private inline fun guard(block: () -> Unit) {
        try {
            block()
        } catch (e: UnsatisfiedLinkError) {
            if (!warned) {
                warned = true
                Log.w(TAG, "NativeBridge call dropped: ${e.message}")
            }
        }
    }
}
