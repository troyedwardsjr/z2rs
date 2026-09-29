package com.z2rs.game

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PadGeometryTest {
    private val density = 2.75f

    @Test
    fun landscapeRightHanded() {
        val l = PadGeometry.compute(2400f, 1080f, density, 1, leftHanded = false, portrait = false)
        assertNull(l.areaTop)
        assertTrue(l.dpad.cx < 1200f)
        assertTrue(l.a.cx > 1200f && l.b.cx > 1200f)
        assertTrue(l.b.cx < l.a.cx) // NES order: B left of A
        assertEquals(PadControl.A, l.hit(l.a.cx, l.a.cy))
        assertEquals(PadControl.B, l.hit(l.b.cx, l.b.cy))
        assertEquals(PadControl.SELECT, l.hit(l.select.cx, l.select.cy))
        assertEquals(PadControl.START, l.hit(l.start.cx, l.start.cy))
        assertEquals(PadControl.DPAD, l.hit(l.dpad.cx, l.dpad.cy))
        assertEquals(PadControl.NONE, l.hit(1200f, 200f)) // the game, not a control
    }

    @Test
    fun leftHandedSwapsSidesButKeepsBLeftOfA() {
        val l = PadGeometry.compute(2400f, 1080f, density, 1, leftHanded = true, portrait = false)
        assertTrue(l.dpad.cx > 1200f)
        assertTrue(l.a.cx < 1200f && l.b.cx < 1200f)
        assertTrue(l.b.cx < l.a.cx)
    }

    @Test
    fun portraitControlsSitBelowTheGame() {
        val l = PadGeometry.compute(1080f, 2400f, density, 1, leftHanded = false, portrait = true)
        val top = l.areaTop!!
        assertTrue(top >= 1200f) // the game keeps at least half the screen
        for (y in listOf(l.dpad.cy - l.dpad.r, l.a.cy - l.a.r, l.b.cy - l.b.r, l.select.top, l.start.top)) {
            assertTrue("control at $y above strip $top", y >= top)
        }
        assertTrue(l.select.bottom <= 2400f)
    }

    @Test
    fun smallestSizeStillHas48dpTargets() {
        val l = PadGeometry.compute(2400f, 1080f, density, 0, leftHanded = false, portrait = false)
        val half = 24f * density
        // A point 23dp from the A centre still hits A even if the drawn circle is smaller.
        assertEquals(PadControl.A, l.hit(l.a.cx + half - 1f, l.a.cy))
        assertEquals(PadControl.SELECT, l.hit(l.select.cx, l.select.cy - half + 1f))
    }

    @Test
    fun dpadBitsFromLayout() {
        val l = PadGeometry.compute(2400f, 1080f, density, 1, leftHanded = false, portrait = false)
        assertEquals(Pad.UP, l.dpadBits(l.dpad.cx, l.dpad.cy - l.dpad.r * 0.8f))
        assertEquals(0, l.dpadBits(l.dpad.cx, l.dpad.cy))
    }
}
