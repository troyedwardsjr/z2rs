package com.z2rs.game

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LaunchArgsTest {
    @Test
    fun buildsDesktopStyleArgv() {
        val args = LaunchArgs.build("/data/user/0/com.z2rs.game/files/rom.nes", "16:9", "integer", coopLocal = true)
        assertEquals(
            listOf(
                "z2-native", "--rom", "/data/user/0/com.z2rs.game/files/rom.nes",
                "--widescreen", "16:9", "--scale-mode", "integer", "--coop-local",
            ),
            args,
        )
    }

    @Test
    fun unknownValuesFallBackToDefaults() {
        val args = LaunchArgs.build("/r.nes", "4:3", "stretch", coopLocal = false)
        assertEquals(listOf("z2-native", "--rom", "/r.nes", "--widescreen", "off", "--scale-mode", "fit"), args)
    }

    @Test
    fun hdPackIsPassedOnlyWhenSet() {
        val pack = "/data/user/0/com.z2rs.game/files/hd_pack/My Pack"
        val args = LaunchArgs.build("/r.nes", "off", "fit", coopLocal = false, hdPack = pack)
        assertEquals(listOf("--hd-pack", pack), args.takeLast(2))
        assertFalse("--hd-pack" in LaunchArgs.build("/r.nes", "off", "fit", coopLocal = false, hdPack = null))
        // A blank path would mean "pack off" to the parser; no pack is no flag.
        assertFalse("--hd-pack" in LaunchArgs.build("/r.nes", "off", "fit", coopLocal = false, hdPack = " "))
    }

    @Test
    fun jsonEscaping() {
        assertEquals("[]", LaunchArgs.toJson(emptyList()))
        assertEquals("""["a","b c"]""", LaunchArgs.toJson(listOf("a", "b c")))
        assertEquals("""["q\"b\\s\n\u0001"]""", LaunchArgs.toJson(listOf("q\"b\\s\n\u0001")))
        assertEquals("""["é"]""", LaunchArgs.toJson(listOf("é")))
    }

    @Test
    fun romSizeSanity() {
        assertTrue(LaunchArgs.plausibleRomSize(262_160))
        assertTrue(LaunchArgs.plausibleRomSize(262_144))
        assertFalse(LaunchArgs.plausibleRomSize(0))
        assertFalse(LaunchArgs.plausibleRomSize(100))
        assertFalse(LaunchArgs.plausibleRomSize(50L * 1024 * 1024))
    }
}
