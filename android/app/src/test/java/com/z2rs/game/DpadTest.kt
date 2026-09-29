package com.z2rs.game

import org.junit.Assert.assertEquals
import org.junit.Test
import kotlin.math.cos
import kotlin.math.sin

class DpadTest {
    private val r = 100f

    @Test
    fun cardinalDirections() {
        assertEquals(Pad.RIGHT, Dpad.bits(80f, 0f, r))
        assertEquals(Pad.LEFT, Dpad.bits(-80f, 0f, r))
        assertEquals(Pad.UP, Dpad.bits(0f, -80f, r)) // screen y grows downwards
        assertEquals(Pad.DOWN, Dpad.bits(0f, 80f, r))
    }

    @Test
    fun diagonals() {
        assertEquals(Pad.UP or Pad.RIGHT, Dpad.bits(60f, -60f, r))
        assertEquals(Pad.UP or Pad.LEFT, Dpad.bits(-60f, -60f, r))
        assertEquals(Pad.DOWN or Pad.RIGHT, Dpad.bits(60f, 60f, r))
        assertEquals(Pad.DOWN or Pad.LEFT, Dpad.bits(-60f, 60f, r))
    }

    @Test
    fun deadZoneInTheMiddle() {
        assertEquals(0, Dpad.bits(0f, 0f, r))
        assertEquals(0, Dpad.bits(19f, 0f, r))
        assertEquals(0, Dpad.bits(-10f, 10f, r))
        assertEquals(Pad.RIGHT, Dpad.bits(21f, 0f, r))
    }

    @Test
    fun fingerOffTheDiscKeepsSteering() {
        assertEquals(Pad.LEFT, Dpad.bits(-500f, 10f, r))
        assertEquals(Pad.DOWN or Pad.LEFT, Dpad.bits(-400f, 400f, r))
    }

    @Test
    fun sectorsAreFortyFiveDegreesWide() {
        // Just either side of the 22.5 degree boundary between RIGHT and DOWN|RIGHT.
        fun at(deg: Double) = Dpad.bits(
            (80 * cos(Math.toRadians(deg))).toFloat(),
            (80 * sin(Math.toRadians(deg))).toFloat(),
            r,
        )
        assertEquals(Pad.RIGHT, at(22.0))
        assertEquals(Pad.RIGHT or Pad.DOWN, at(23.0))
        assertEquals(Pad.RIGHT or Pad.DOWN, at(67.0))
        assertEquals(Pad.DOWN, at(68.0))
        assertEquals(Pad.LEFT, at(179.0))
        assertEquals(Pad.LEFT, at(-179.0))
        // Every angle gives one direction or a diagonal pair, never opposites.
        for (deg in 0 until 360) {
            val m = at(deg.toDouble())
            assert(m != 0)
            assert(m and (Pad.UP or Pad.DOWN) != (Pad.UP or Pad.DOWN))
            assert(m and (Pad.LEFT or Pad.RIGHT) != (Pad.LEFT or Pad.RIGHT))
        }
    }

    @Test
    fun zeroRadiusIsSafe() {
        assertEquals(0, Dpad.bits(10f, 10f, 0f))
    }
}
