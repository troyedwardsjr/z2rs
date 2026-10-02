package com.z2rs.game

/**
 * The argv the Rust side feeds to z2-native's parse_native_args, written by
 * the launcher to filesDir/launch_args.json as a JSON array of strings
 * (argv[0] included). Only flags that parser accepts and that make sense on a
 * phone are ever written. Pure Kotlin, unit-tested.
 */
object LaunchArgs {
    const val FILE_NAME = "launch_args.json"
    const val ARGV0 = "z2-native"

    /** `--widescreen` presets parse_native_args accepts (z2_ppu::preset_tiles). */
    val WIDESCREEN_VALUES = listOf("off", "16:10", "16:9", "21:9")

    /** `--scale-mode` values (ScaleMode::parse). */
    val SCALE_MODES = listOf("fit", "integer")

    fun build(
        romPath: String,
        widescreen: String,
        scaleMode: String,
        coopLocal: Boolean,
        hdPack: String? = null,
    ): List<String> {
        val args = mutableListOf(ARGV0, "--rom", romPath)
        args += listOf("--widescreen", if (widescreen in WIDESCREEN_VALUES) widescreen else "off")
        args += listOf("--scale-mode", if (scaleMode in SCALE_MODES) scaleMode else "fit")
        if (coopLocal) args += "--coop-local"
        // `--hd-pack ''` would turn a pack off; no pack is simply no flag.
        if (!hdPack.isNullOrBlank()) args += listOf("--hd-pack", hdPack)
        return args
    }

    /** A JSON array of strings, escaped per RFC 8259. */
    fun toJson(args: List<String>): String {
        val sb = StringBuilder("[")
        args.forEachIndexed { i, s ->
            if (i > 0) sb.append(',')
            sb.append('"')
            for (c in s) {
                when (c) {
                    '"' -> sb.append("\\\"")
                    '\\' -> sb.append("\\\\")
                    '\n' -> sb.append("\\n")
                    '\r' -> sb.append("\\r")
                    '\t' -> sb.append("\\t")
                    '\b' -> sb.append("\\b")
                    '\u000C' -> sb.append("\\f")
                    else -> if (c < ' ') {
                        sb.append(String.format("\\u%04x", c.code))
                    } else {
                        sb.append(c)
                    }
                }
            }
            sb.append('"')
        }
        return sb.append(']').toString()
    }

    /** ROM size sanity only (Rust does the real CRC/SHA-1 check): 16 KiB to 4 MiB. */
    fun plausibleRomSize(bytes: Long): Boolean = bytes in 16L * 1024..4L * 1024 * 1024
}
