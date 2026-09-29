package com.z2rs.game

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class ControllerRegistryTest {
    private val sent = mutableListOf<Pair<Int, Int>>()
    private val reg = ControllerRegistry { p, m -> sent += p to m }

    @Test
    fun playersFollowDeviceIdOrder() {
        reg.connect(12)
        reg.connect(5)
        reg.connect(30)
        assertEquals(0, reg.playerOf(5))
        assertEquals(1, reg.playerOf(12))
        assertNull(reg.playerOf(30))
    }

    @Test
    fun sendsOnlyOnChange() {
        reg.connect(7)
        reg.onKey(7, Pad.A, true)
        reg.onKey(7, Pad.A, true)
        reg.onAxes(7, Pad.LEFT)
        reg.onAxes(7, Pad.LEFT)
        reg.onKey(7, Pad.A, false)
        assertEquals(listOf(0 to Pad.A, 0 to (Pad.A or Pad.LEFT), 0 to Pad.LEFT), sent)
    }

    @Test
    fun secondControllerIsPlayerTwo() {
        reg.onKey(3, Pad.START, true)
        reg.onKey(9, Pad.B, true)
        assertEquals(listOf(0 to Pad.START, 1 to Pad.B), sent)
    }

    @Test
    fun disconnectZeroesAndReassigns() {
        reg.onKey(3, Pad.A, true)
        reg.onKey(9, Pad.B, true)
        sent.clear()
        reg.disconnect(3)
        // Device 9 is now the only controller, so it becomes player 0.
        assertEquals(listOf(0 to Pad.B, 1 to 0), sent)
    }

    @Test
    fun releaseAllKeepsSlots() {
        reg.onKey(3, Pad.A or Pad.UP, true)
        reg.releaseAll()
        assertEquals(0, reg.maskOf(0))
        assertEquals(1, reg.count)
        assertEquals(0 to 0, sent.last())
    }
}
