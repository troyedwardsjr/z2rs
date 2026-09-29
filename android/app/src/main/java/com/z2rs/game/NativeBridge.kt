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
}

/**
 * Guarded calls into [NativeBridge]. An UnsatisfiedLinkError (library not
 * loaded, or a symbol the Rust side does not export) is logged once and the
 * call is dropped, so the UI can never crash on it.
 */
object Native {
    private const val TAG = "z2rs"
    @Volatile private var warned = false

    fun setTouchPad(mask: Int) = guard { NativeBridge.setTouchPad(mask and 0xFF) }
    fun setHardwarePad(player: Int, mask: Int) = guard { NativeBridge.setHardwarePad(player, mask and 0xFF) }
    fun setPaused(paused: Boolean) = guard { NativeBridge.setPaused(paused) }
    fun requestSaveState(slot: Int) = guard { NativeBridge.requestSaveState(slot) }
    fun requestLoadState(slot: Int) = guard { NativeBridge.requestLoadState(slot) }

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
