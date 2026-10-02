package com.z2rs.game

import android.content.Context
import android.content.SharedPreferences

/**
 * Launcher settings, in SharedPreferences. Written with commit() rather than
 * apply(): the game runs in its own process and reads them once at start, so
 * a change must be on disk before Play starts that process.
 */
class Settings(context: Context) {
    private val prefs: SharedPreferences =
        context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)

    var touchEnabled: Boolean
        get() = prefs.getBoolean(K_TOUCH, true)
        set(v) = put { putBoolean(K_TOUCH, v) }

    var hideTouchWithController: Boolean
        get() = prefs.getBoolean(K_HIDE_WITH_PAD, true)
        set(v) = put { putBoolean(K_HIDE_WITH_PAD, v) }

    /** 0 = small, 1 = medium, 2 = large. */
    var padSize: Int
        get() = prefs.getInt(K_SIZE, 1).coerceIn(0, 2)
        set(v) = put { putInt(K_SIZE, v.coerceIn(0, 2)) }

    /** Percent, 20..100; applies where the pad floats over the game. */
    var padOpacity: Int
        get() = prefs.getInt(K_OPACITY, 60).coerceIn(MIN_OPACITY, 100)
        set(v) = put { putInt(K_OPACITY, v.coerceIn(MIN_OPACITY, 100)) }

    var haptics: Boolean
        get() = prefs.getBoolean(K_HAPTICS, true)
        set(v) = put { putBoolean(K_HAPTICS, v) }

    var leftHanded: Boolean
        get() = prefs.getBoolean(K_LEFT, false)
        set(v) = put { putBoolean(K_LEFT, v) }

    var highContrast: Boolean
        get() = prefs.getBoolean(K_CONTRAST, false)
        set(v) = put { putBoolean(K_CONTRAST, v) }

    var keepScreenOn: Boolean
        get() = prefs.getBoolean(K_SCREEN_ON, true)
        set(v) = put { putBoolean(K_SCREEN_ON, v) }

    var widescreen: String
        get() = prefs.getString(K_WIDESCREEN, "off")!!.takeIf { it in LaunchArgs.WIDESCREEN_VALUES } ?: "off"
        set(v) = put { putString(K_WIDESCREEN, v) }

    var scaleMode: String
        get() = prefs.getString(K_SCALE_MODE, "fit")!!.takeIf { it in LaunchArgs.SCALE_MODES } ?: "fit"
        set(v) = put { putString(K_SCALE_MODE, v) }

    var coopLocal: Boolean
        get() = prefs.getBoolean(K_COOP, false)
        set(v) = put { putBoolean(K_COOP, v) }

    /** Display name of the picked ROM file, for the launcher status line only. */
    var romName: String?
        get() = prefs.getString(K_ROM_NAME, null)
        set(v) = put { putString(K_ROM_NAME, v) }

    /**
     * The imported HD pack: its pack.json folder relative to filesDir/hd_pack
     * (`""` or ending in `/`), or null for none.
     */
    val hdPackRoot: String?
        get() = prefs.getString(K_HD_ROOT, null)

    /** The pack's name, for the launcher status line. */
    val hdPackName: String?
        get() = prefs.getString(K_HD_NAME, null)

    /** Tiles the pack replaces, or -1 when it was not checked at import. */
    val hdPackTiles: Int
        get() = prefs.getInt(K_HD_TILES, -1)

    fun setHdPack(root: String, name: String, tiles: Int) = put {
        putString(K_HD_ROOT, root)
        putString(K_HD_NAME, name)
        putInt(K_HD_TILES, tiles)
    }

    fun clearHdPack() = put {
        remove(K_HD_ROOT)
        remove(K_HD_NAME)
        remove(K_HD_TILES)
    }

    private inline fun put(block: SharedPreferences.Editor.() -> Unit) {
        prefs.edit().apply(block).commit()
    }

    companion object {
        const val FILE = "z2rs_settings"
        const val MIN_OPACITY = 20
        private const val K_TOUCH = "touch_enabled"
        private const val K_HIDE_WITH_PAD = "touch_hide_with_controller"
        private const val K_SIZE = "pad_size"
        private const val K_OPACITY = "pad_opacity"
        private const val K_HAPTICS = "haptics"
        private const val K_LEFT = "left_handed"
        private const val K_CONTRAST = "high_contrast"
        private const val K_SCREEN_ON = "keep_screen_on"
        private const val K_WIDESCREEN = "widescreen"
        private const val K_SCALE_MODE = "scale_mode"
        private const val K_COOP = "coop_local"
        private const val K_ROM_NAME = "rom_name"
        private const val K_HD_ROOT = "hd_pack_root"
        private const val K_HD_NAME = "hd_pack_name"
        private const val K_HD_TILES = "hd_pack_tiles"
    }
}
