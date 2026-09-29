package com.z2rs.game

import kotlin.math.PI
import kotlin.math.atan2
import kotlin.math.hypot
import kotlin.math.roundToInt

// Pure input logic: no Android types, so it runs in plain JVM unit tests.

/** NES pad byte bits, LSB first. Shared with the Rust side (NativeBridge). */
object Pad {
    const val A = 1 shl 0
    const val B = 1 shl 1
    const val SELECT = 1 shl 2
    const val START = 1 shl 3
    const val UP = 1 shl 4
    const val DOWN = 1 shl 5
    const val LEFT = 1 shl 6
    const val RIGHT = 1 shl 7
    const val DIRS = UP or DOWN or LEFT or RIGHT
}

/**
 * The on-screen d-pad: eight equal 45-degree sectors round the centre with a
 * small dead zone in the middle, the same as the web build's touch pad. There
 * is no outer limit, so a finger that started on the d-pad keeps steering
 * after it drifts off the disc.
 */
object Dpad {
    /** Dead zone as a fraction of the d-pad radius. */
    const val DEAD_ZONE = 0.2f

    /** Clockwise from east; screen y grows downwards, like atan2 on screen coordinates. */
    private val OCTANTS = intArrayOf(
        Pad.RIGHT,
        Pad.RIGHT or Pad.DOWN,
        Pad.DOWN,
        Pad.DOWN or Pad.LEFT,
        Pad.LEFT,
        Pad.LEFT or Pad.UP,
        Pad.UP,
        Pad.UP or Pad.RIGHT,
    )

    /**
     * Direction bits for a touch at ([dx], [dy]) from the d-pad centre (screen
     * pixels, y down) on a d-pad of [radius] pixels.
     */
    fun bits(dx: Float, dy: Float, radius: Float, deadZone: Float = DEAD_ZONE): Int {
        if (radius <= 0f) return 0
        val nx = dx / radius
        val ny = dy / radius
        if (hypot(nx, ny) < deadZone) return 0
        val oct = (atan2(ny, nx) / (PI.toFloat() / 4f)).roundToInt()
        return OCTANTS[(oct % 8 + 8) % 8]
    }
}

/**
 * Hardware controller mapping. Key codes and axis ids are Android's
 * (android.view.KeyEvent / MotionEvent) values, repeated here so the mapping
 * stays testable without the Android framework; GamepadMappingTest checks
 * them against the framework constants.
 *
 * Face buttons are positional, like the desktop build (gilrs East -> A,
 * South -> B): Android's KEYCODE_BUTTON_B is the right-hand (east) button and
 * KEYCODE_BUTTON_A the bottom (south) one, so the button in the NES A
 * position is A whatever its label says.
 */
object GamepadMapping {
    const val KEYCODE_BACK = 4
    const val KEYCODE_DPAD_UP = 19
    const val KEYCODE_DPAD_DOWN = 20
    const val KEYCODE_DPAD_LEFT = 21
    const val KEYCODE_DPAD_RIGHT = 22
    const val KEYCODE_MENU = 82
    const val KEYCODE_BUTTON_A = 96
    const val KEYCODE_BUTTON_B = 97
    const val KEYCODE_BUTTON_X = 99
    const val KEYCODE_BUTTON_Y = 100
    const val KEYCODE_BUTTON_START = 108
    const val KEYCODE_BUTTON_SELECT = 109
    const val KEYCODE_BUTTON_MODE = 110
    const val KEYCODE_DPAD_UP_LEFT = 268
    const val KEYCODE_DPAD_DOWN_LEFT = 269
    const val KEYCODE_DPAD_UP_RIGHT = 270
    const val KEYCODE_DPAD_DOWN_RIGHT = 271

    const val AXIS_X = 0
    const val AXIS_Y = 1
    const val AXIS_HAT_X = 15
    const val AXIS_HAT_Y = 16

    /** Analog stick and hat threshold. */
    const val STICK_THRESHOLD = 0.5f

    /** Pad bits for a controller key code, or 0 when the key is not a pad input. */
    fun keyBits(keyCode: Int): Int = when (keyCode) {
        KEYCODE_BUTTON_B -> Pad.A // east
        KEYCODE_BUTTON_A -> Pad.B // south
        KEYCODE_BUTTON_SELECT -> Pad.SELECT
        KEYCODE_BUTTON_START -> Pad.START
        KEYCODE_DPAD_UP -> Pad.UP
        KEYCODE_DPAD_DOWN -> Pad.DOWN
        KEYCODE_DPAD_LEFT -> Pad.LEFT
        KEYCODE_DPAD_RIGHT -> Pad.RIGHT
        KEYCODE_DPAD_UP_LEFT -> Pad.UP or Pad.LEFT
        KEYCODE_DPAD_UP_RIGHT -> Pad.UP or Pad.RIGHT
        KEYCODE_DPAD_DOWN_LEFT -> Pad.DOWN or Pad.LEFT
        KEYCODE_DPAD_DOWN_RIGHT -> Pad.DOWN or Pad.RIGHT
        else -> 0
    }

    /** Keys on a controller that open the pause menu instead of reaching the game. */
    fun isMenuKey(keyCode: Int): Boolean =
        keyCode == KEYCODE_BUTTON_MODE || keyCode == KEYCODE_MENU

    /** Direction bits for one axis pair (negative = left/up). */
    fun axisBits(x: Float, y: Float, threshold: Float = STICK_THRESHOLD): Int {
        var m = 0
        if (x <= -threshold) m = m or Pad.LEFT
        if (x >= threshold) m = m or Pad.RIGHT
        if (y <= -threshold) m = m or Pad.UP
        if (y >= threshold) m = m or Pad.DOWN
        return m
    }

    /** Direction bits for a joystick move: left stick OR hat switch. */
    fun motionBits(x: Float, y: Float, hatX: Float, hatY: Float): Int =
        axisBits(x, y) or axisBits(hatX, hatY)
}

/**
 * Hardware controllers by device id. Connected controllers are ordered by
 * device id: the first is player 0, the second player 1, any more are
 * ignored. Each player's mask is the OR of its device's key and stick bits,
 * and [sink] hears about it only when it changes.
 */
class ControllerRegistry(private val sink: (player: Int, mask: Int) -> Unit) {
    private val connected = sortedSetOf<Int>()
    private val keyMask = HashMap<Int, Int>()
    private val axisMask = HashMap<Int, Int>()
    private val sent = IntArray(MAX_PLAYERS)

    val count: Int get() = connected.size

    fun deviceIds(): List<Int> = connected.toList()

    /** Player index (0 or 1) for [deviceId], or null when it has no slot. */
    fun playerOf(deviceId: Int): Int? {
        val i = connected.indexOf(deviceId)
        return if (i in 0 until MAX_PLAYERS) i else null
    }

    /** Returns true when the device was not known before. */
    fun connect(deviceId: Int): Boolean {
        val added = connected.add(deviceId)
        if (added) publish()
        return added
    }

    fun disconnect(deviceId: Int) {
        connected.remove(deviceId)
        keyMask.remove(deviceId)
        axisMask.remove(deviceId)
        publish()
    }

    fun onKey(deviceId: Int, bits: Int, down: Boolean) {
        connect(deviceId)
        val old = keyMask[deviceId] ?: 0
        keyMask[deviceId] = if (down) old or bits else old and bits.inv()
        publish()
    }

    fun onAxes(deviceId: Int, bits: Int) {
        connect(deviceId)
        axisMask[deviceId] = bits
        publish()
    }

    /** Releases every button (focus loss, pause) but keeps the player slots. */
    fun releaseAll() {
        keyMask.clear()
        axisMask.clear()
        publish()
    }

    fun maskOf(player: Int): Int {
        var m = 0
        for (id in connected) {
            if (playerOf(id) == player) m = m or (keyMask[id] ?: 0) or (axisMask[id] ?: 0)
        }
        return m
    }

    private fun publish() {
        for (p in 0 until MAX_PLAYERS) {
            val m = maskOf(p)
            if (m != sent[p]) {
                sent[p] = m
                sink(p, m)
            }
        }
    }

    companion object {
        const val MAX_PLAYERS = 2
    }
}
