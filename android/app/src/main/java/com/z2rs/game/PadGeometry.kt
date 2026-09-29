package com.z2rs.game

import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min

// Where the on-screen gamepad's controls go, in view pixels. Pure Kotlin.

data class Circle(val cx: Float, val cy: Float, val r: Float) {
    fun contains(x: Float, y: Float, hitR: Float = r) = hypot(x - cx, y - cy) <= hitR
}

data class Box(val left: Float, val top: Float, val right: Float, val bottom: Float) {
    val cx get() = (left + right) / 2f
    val cy get() = (top + bottom) / 2f
    val width get() = right - left
    val height get() = bottom - top
    fun contains(x: Float, y: Float) = x in left..right && y in top..bottom

    /** Grown symmetrically to at least [minW] x [minH]. */
    fun atLeast(minW: Float, minH: Float): Box {
        val gx = max(0f, (minW - width) / 2f)
        val gy = max(0f, (minH - height) / 2f)
        return Box(left - gx, top - gy, right + gx, bottom + gy)
    }
}

data class Insets4(val left: Float = 0f, val top: Float = 0f, val right: Float = 0f, val bottom: Float = 0f) {
    companion object { val NONE = Insets4() }
}

/** Which control a point hits. */
enum class PadControl { NONE, DPAD, SELECT, START, B, A }

class PadLayout(
    val dpad: Circle,
    val a: Circle,
    val b: Circle,
    val select: Box,
    val start: Box,
    /** Minimum hit radius / size in px (48dp touch target). */
    val minTarget: Float,
    /** Portrait: top of the control strip below the game. Landscape: null. */
    val areaTop: Float?,
) {
    private val selectHit = select.atLeast(minTarget, minTarget)
    private val startHit = start.atLeast(minTarget, minTarget)

    /** Hit test in the order a finger should resolve: face buttons, then Select/Start, then the d-pad. */
    fun hit(x: Float, y: Float): PadControl {
        val half = minTarget / 2f
        return when {
            a.contains(x, y, max(a.r, half)) -> PadControl.A
            b.contains(x, y, max(b.r, half)) -> PadControl.B
            selectHit.contains(x, y) -> PadControl.SELECT
            startHit.contains(x, y) -> PadControl.START
            dpad.contains(x, y, max(dpad.r, half)) -> PadControl.DPAD
            else -> PadControl.NONE
        }
    }

    /** Bits for a finger that is not steering the d-pad. */
    fun buttonBits(x: Float, y: Float): Int = when (hit(x, y)) {
        PadControl.A -> Pad.A
        PadControl.B -> Pad.B
        PadControl.SELECT -> Pad.SELECT
        PadControl.START -> Pad.START
        else -> 0
    }

    fun dpadBits(x: Float, y: Float): Int = Dpad.bits(x - dpad.cx, y - dpad.cy, dpad.r)

    /** True for a point inside the portrait control strip (touches there never reach the game). */
    fun inArea(y: Float): Boolean = areaTop != null && y >= areaTop
}

object PadGeometry {
    /** Size multipliers for small / medium / large. */
    val SIZE_SCALE = floatArrayOf(0.8f, 1.0f, 1.25f)

    private const val DPAD_R_DP = 64f
    private const val BTN_R_DP = 30f
    private const val PILL_W_DP = 60f
    private const val PILL_H_DP = 26f
    private const val PILL_GAP_DP = 18f
    private const val MARGIN_DP = 20f
    const val MIN_TARGET_DP = 48f

    private fun scale(sizeIndex: Int) = SIZE_SCALE[sizeIndex.coerceIn(0, SIZE_SCALE.size - 1)]

    /**
     * Height of the portrait control strip (px), excluding the bottom inset,
     * capped at half the screen so the game keeps the other half.
     */
    fun portraitAreaHeight(height: Float, density: Float, sizeIndex: Int): Float {
        val s = scale(sizeIndex) * density
        val clusters = max(2f * DPAD_R_DP, 2.6f * BTN_R_DP) * s
        val needed = clusters + PILL_H_DP * s + 3f * MARGIN_DP * density + 24f * density
        return min(needed, height * 0.5f)
    }

    fun compute(
        width: Float,
        height: Float,
        density: Float,
        sizeIndex: Int,
        leftHanded: Boolean,
        portrait: Boolean,
        insets: Insets4 = Insets4.NONE,
    ): PadLayout {
        val s = scale(sizeIndex) * density
        val m = MARGIN_DP * density
        val side = max(insets.left, insets.right)
        val dpadR = DPAD_R_DP * s
        val btnR = BTN_R_DP * s
        val pillW = PILL_W_DP * s
        val pillH = PILL_H_DP * s
        val pillGap = PILL_GAP_DP * s

        val areaTop: Float?
        val clusterY: Float
        val pillY: Float
        if (portrait) {
            val area = portraitAreaHeight(height, density, sizeIndex)
            val top = height - insets.bottom - area
            areaTop = top
            // Select/Start on the bottom row, the clusters centred above it.
            pillY = height - insets.bottom - m - pillH / 2f
            val clustersBottom = pillY - pillH / 2f - m * 0.5f
            clusterY = max(top + m + dpadR, (top + clustersBottom) / 2f)
        } else {
            areaTop = null
            clusterY = height - insets.bottom - m - dpadR
            pillY = height - insets.bottom - m * 0.6f - pillH / 2f
        }

        // Right-handed: d-pad left, B/A right. B sits left of and a little below A.
        var dpadX = side + m + dpadR
        val abOffset = 2.4f * btnR
        var aX = width - side - m - btnR
        val cc0 = aX - abOffset / 2f
        if (leftHanded) {
            dpadX = width - dpadX
            aX = (width - cc0) + abOffset / 2f
        }
        val bX = aX - abOffset
        val a = Circle(aX, clusterY - 0.3f * btnR, btnR)
        val b = Circle(bX, clusterY + 0.3f * btnR, btnR)
        val dpad = Circle(dpadX, clusterY, dpadR)

        val cx = width / 2f
        val select = Box(cx - pillGap / 2f - pillW, pillY - pillH / 2f, cx - pillGap / 2f, pillY + pillH / 2f)
        val start = Box(cx + pillGap / 2f, pillY - pillH / 2f, cx + pillGap / 2f + pillW, pillY + pillH / 2f)

        return PadLayout(dpad, a, b, select, start, MIN_TARGET_DP * density, areaTop)
    }
}
