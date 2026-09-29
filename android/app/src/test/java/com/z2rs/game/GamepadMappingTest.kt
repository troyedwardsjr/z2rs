package com.z2rs.game

import android.view.KeyEvent
import android.view.MotionEvent
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class GamepadMappingTest {
    @Test
    fun padBitsMatchTheRustContract() {
        assertEquals(0x01, Pad.A)
        assertEquals(0x02, Pad.B)
        assertEquals(0x04, Pad.SELECT)
        assertEquals(0x08, Pad.START)
        assertEquals(0x10, Pad.UP)
        assertEquals(0x20, Pad.DOWN)
        assertEquals(0x40, Pad.LEFT)
        assertEquals(0x80, Pad.RIGHT)
    }

    @Test
    fun keyCodesMatchTheFramework() {
        // Framework constants are compile-time inlined, so this runs on the JVM.
        assertEquals(KeyEvent.KEYCODE_BACK, GamepadMapping.KEYCODE_BACK)
        assertEquals(KeyEvent.KEYCODE_DPAD_UP, GamepadMapping.KEYCODE_DPAD_UP)
        assertEquals(KeyEvent.KEYCODE_DPAD_DOWN, GamepadMapping.KEYCODE_DPAD_DOWN)
        assertEquals(KeyEvent.KEYCODE_DPAD_LEFT, GamepadMapping.KEYCODE_DPAD_LEFT)
        assertEquals(KeyEvent.KEYCODE_DPAD_RIGHT, GamepadMapping.KEYCODE_DPAD_RIGHT)
        assertEquals(KeyEvent.KEYCODE_MENU, GamepadMapping.KEYCODE_MENU)
        assertEquals(KeyEvent.KEYCODE_BUTTON_A, GamepadMapping.KEYCODE_BUTTON_A)
        assertEquals(KeyEvent.KEYCODE_BUTTON_B, GamepadMapping.KEYCODE_BUTTON_B)
        assertEquals(KeyEvent.KEYCODE_BUTTON_X, GamepadMapping.KEYCODE_BUTTON_X)
        assertEquals(KeyEvent.KEYCODE_BUTTON_Y, GamepadMapping.KEYCODE_BUTTON_Y)
        assertEquals(KeyEvent.KEYCODE_BUTTON_START, GamepadMapping.KEYCODE_BUTTON_START)
        assertEquals(KeyEvent.KEYCODE_BUTTON_SELECT, GamepadMapping.KEYCODE_BUTTON_SELECT)
        assertEquals(KeyEvent.KEYCODE_BUTTON_MODE, GamepadMapping.KEYCODE_BUTTON_MODE)
        assertEquals(KeyEvent.KEYCODE_DPAD_UP_LEFT, GamepadMapping.KEYCODE_DPAD_UP_LEFT)
        assertEquals(KeyEvent.KEYCODE_DPAD_DOWN_LEFT, GamepadMapping.KEYCODE_DPAD_DOWN_LEFT)
        assertEquals(KeyEvent.KEYCODE_DPAD_UP_RIGHT, GamepadMapping.KEYCODE_DPAD_UP_RIGHT)
        assertEquals(KeyEvent.KEYCODE_DPAD_DOWN_RIGHT, GamepadMapping.KEYCODE_DPAD_DOWN_RIGHT)
        assertEquals(MotionEvent.AXIS_X, GamepadMapping.AXIS_X)
        assertEquals(MotionEvent.AXIS_Y, GamepadMapping.AXIS_Y)
        assertEquals(MotionEvent.AXIS_HAT_X, GamepadMapping.AXIS_HAT_X)
        assertEquals(MotionEvent.AXIS_HAT_Y, GamepadMapping.AXIS_HAT_Y)
    }

    @Test
    fun faceButtonsArePositionalLikeTheDesktop() {
        // East (KEYCODE_BUTTON_B) is NES A; south (KEYCODE_BUTTON_A) is NES B.
        assertEquals(Pad.A, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_B))
        assertEquals(Pad.B, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_A))
        assertEquals(Pad.SELECT, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_SELECT))
        assertEquals(Pad.START, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_START))
        assertEquals(0, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_X))
        assertEquals(0, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_Y))
        assertEquals(0, GamepadMapping.keyBits(KeyEvent.KEYCODE_BUTTON_L1))
    }

    @Test
    fun dpadKeys() {
        assertEquals(Pad.UP, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_UP))
        assertEquals(Pad.DOWN, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_DOWN))
        assertEquals(Pad.LEFT, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_LEFT))
        assertEquals(Pad.RIGHT, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_RIGHT))
        assertEquals(Pad.UP or Pad.LEFT, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_UP_LEFT))
        assertEquals(Pad.DOWN or Pad.RIGHT, GamepadMapping.keyBits(KeyEvent.KEYCODE_DPAD_DOWN_RIGHT))
        assertEquals(0, GamepadMapping.keyBits(KeyEvent.KEYCODE_A)) // keyboard keys are not ours
    }

    @Test
    fun menuKeys() {
        assertTrue(GamepadMapping.isMenuKey(KeyEvent.KEYCODE_BUTTON_MODE))
        assertTrue(GamepadMapping.isMenuKey(KeyEvent.KEYCODE_MENU))
        assertFalse(GamepadMapping.isMenuKey(KeyEvent.KEYCODE_BUTTON_START))
    }

    @Test
    fun axesUseAHalfDeflectionThreshold() {
        assertEquals(0, GamepadMapping.axisBits(0.49f, -0.49f))
        assertEquals(Pad.RIGHT, GamepadMapping.axisBits(0.5f, 0f))
        assertEquals(Pad.LEFT, GamepadMapping.axisBits(-0.8f, 0.1f))
        assertEquals(Pad.UP, GamepadMapping.axisBits(0f, -1f))
        assertEquals(Pad.DOWN or Pad.LEFT, GamepadMapping.axisBits(-0.7f, 0.7f))
    }

    @Test
    fun stickAndHatCombine() {
        assertEquals(Pad.UP or Pad.RIGHT, GamepadMapping.motionBits(0f, -1f, 1f, 0f))
        assertEquals(Pad.DOWN, GamepadMapping.motionBits(0.1f, 0.1f, 0f, 1f))
        assertEquals(0, GamepadMapping.motionBits(0f, 0f, 0f, 0f))
    }
}
