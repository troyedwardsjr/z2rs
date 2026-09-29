package com.z2rs.game

import android.content.Context
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Path
import android.graphics.Rect
import android.graphics.RectF
import android.graphics.Typeface
import android.os.Bundle
import android.util.SparseArray
import android.view.HapticFeedbackConstants
import android.view.MotionEvent
import android.view.View
import androidx.core.view.ViewCompat
import androidx.core.view.accessibility.AccessibilityNodeInfoCompat
import androidx.customview.widget.ExploreByTouchHelper
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The on-screen gamepad for player 1: a d-pad, Select, Start, B and A, drawn
 * with Canvas over (landscape) or below (portrait) the game.
 *
 * Input matches the web build's touch pad: every finger is tracked by pointer
 * id and re-hit-tested as it moves, so a thumb can roll from B onto A without
 * lifting; the d-pad is eight-way with a small dead zone; and a finger that
 * started on the d-pad keeps steering after it drifts off the disc. The
 * combined mask goes to [Native.setTouchPad] only when it changes.
 *
 * A first touch that lands on no control returns false, so it passes through
 * to the game surface underneath.
 *
 * Accessibility: an [ExploreByTouchHelper] exposes Up, Down, Left, Right,
 * Select, Start, B and A as labelled virtual views; activating one (TalkBack
 * double-tap, Switch Access select) presses it for [PULSE_MS].
 */
class TouchPadView(context: Context) : View(context) {

    private class Finger(val dpad: Boolean, var bits: Int)

    private val fingers = SparseArray<Finger>()
    private var pulseMask = 0
    private var sentMask = 0
    private var shownMask = 0

    private var layoutInfo: PadLayout? = null
    private var portrait = false
    private var safeInsets = Insets4.NONE

    // Settings
    private var sizeIndex = 1
    private var leftHanded = false
    private var highContrast = false
    private var hapticsOn = true
    private var overlayAlpha = 0.6f

    private val density = resources.displayMetrics.density
    private val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.FILL }
    private val stroke = Paint(Paint.ANTI_ALIAS_FLAG).apply { style = Paint.Style.STROKE }
    private val text = Paint(Paint.ANTI_ALIAS_FLAG).apply {
        textAlign = Paint.Align.CENTER
        typeface = Typeface.create(Typeface.DEFAULT, Typeface.BOLD)
    }
    private val path = Path()
    private val rectF = RectF()

    private val a11y = PadAccessibility()

    init {
        isFocusable = false
        isFocusableInTouchMode = false
        isClickable = false
        contentDescription = context.getString(R.string.touch_pad_desc)
        importantForAccessibility = IMPORTANT_FOR_ACCESSIBILITY_YES
        ViewCompat.setAccessibilityDelegate(this, a11y)
    }

    fun configure(settings: Settings) {
        sizeIndex = settings.padSize
        leftHanded = settings.leftHanded
        highContrast = settings.highContrast
        hapticsOn = settings.haptics
        isHapticFeedbackEnabled = hapticsOn
        overlayAlpha = settings.padOpacity / 100f
        relayoutControls()
    }

    /** Called by GameActivity when orientation or the cutout insets change. */
    fun setFrame(portrait: Boolean, insets: Insets4) {
        if (portrait == this.portrait && insets == safeInsets && layoutInfo != null) return
        this.portrait = portrait
        this.safeInsets = insets
        relayoutControls()
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        relayoutControls()
    }

    private fun relayoutControls() {
        if (width == 0 || height == 0) {
            layoutInfo = null
            return
        }
        layoutInfo = PadGeometry.compute(
            width.toFloat(), height.toFloat(), density, sizeIndex, leftHanded, portrait, safeInsets,
        )
        // Fingers were hit-tested against the old geometry.
        releaseAll()
        a11y.invalidateRoot()
        invalidate()
    }

    // --- touch ---------------------------------------------------------------

    override fun onTouchEvent(event: MotionEvent): Boolean {
        val l = layoutInfo ?: return false
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN, MotionEvent.ACTION_POINTER_DOWN -> {
                val i = event.actionIndex
                val x = event.getX(i)
                val y = event.getY(i)
                val hit = l.hit(x, y)
                if (hit == PadControl.NONE && event.actionMasked == MotionEvent.ACTION_DOWN && !l.inArea(y)) {
                    return false // not ours: let the game have it
                }
                val dpad = hit == PadControl.DPAD
                fingers.put(event.getPointerId(i), Finger(dpad, if (dpad) l.dpadBits(x, y) else l.buttonBits(x, y)))
                update()
            }
            MotionEvent.ACTION_MOVE -> {
                var changed = false
                for (i in 0 until event.pointerCount) {
                    val f = fingers.get(event.getPointerId(i)) ?: continue
                    val x = event.getX(i)
                    val y = event.getY(i)
                    val bits = if (f.dpad) l.dpadBits(x, y) else l.buttonBits(x, y)
                    if (bits != f.bits) {
                        f.bits = bits
                        changed = true
                    }
                }
                if (changed) update()
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_POINTER_UP -> {
                fingers.remove(event.getPointerId(event.actionIndex))
                update()
            }
            MotionEvent.ACTION_CANCEL -> {
                fingers.clear()
                update()
            }
        }
        return true
    }

    /** Releases every finger and pulse and sends 0 (pause, focus loss, hide, detach). */
    fun releaseAll() {
        fingers.clear()
        pulseMask = 0
        removeCallbacks(pulseEnd)
        update()
    }

    private fun currentMask(): Int {
        var m = pulseMask
        for (i in 0 until fingers.size()) m = m or fingers.valueAt(i).bits
        return m
    }

    private fun update() {
        val m = currentMask()
        if (m != sentMask) {
            val pressed = m and sentMask.inv()
            sentMask = m
            Native.setTouchPad(m)
            if (pressed != 0 && hapticsOn) {
                // Honours the system touch-feedback setting (no FLAG_IGNORE_GLOBAL_SETTING).
                performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
            }
        }
        if (m != shownMask) {
            shownMask = m
            invalidate()
        }
    }

    override fun setVisibility(visibility: Int) {
        if (visibility != VISIBLE) releaseAll()
        super.setVisibility(visibility)
    }

    override fun onDetachedFromWindow() {
        releaseAll()
        super.onDetachedFromWindow()
    }

    // --- accessibility pulses ---------------------------------------------------

    private val pulseEnd = Runnable {
        pulseMask = 0
        update()
    }

    /** Press [bits] for [PULSE_MS], then release (TalkBack / Switch Access activation). */
    fun pulse(bits: Int) {
        pulseMask = pulseMask or bits
        update()
        removeCallbacks(pulseEnd)
        postDelayed(pulseEnd, PULSE_MS)
    }

    override fun dispatchHoverEvent(event: MotionEvent): Boolean =
        a11y.dispatchHoverEvent(event) || super.dispatchHoverEvent(event)

    // --- drawing -------------------------------------------------------------

    override fun onDraw(canvas: Canvas) {
        val l = layoutInfo ?: return
        val alpha = if (portrait) 1f else overlayAlpha
        val saved = if (alpha < 1f) {
            canvas.saveLayerAlpha(0f, 0f, width.toFloat(), height.toFloat(), (alpha * 255).roundToInt())
        } else {
            canvas.save()
        }
        drawDpad(canvas, l.dpad, shownMask)
        drawRound(canvas, l.b, shownMask and Pad.B != 0, context.getString(R.string.pad_label_b))
        drawRound(canvas, l.a, shownMask and Pad.A != 0, context.getString(R.string.pad_label_a))
        drawPill(canvas, l.select, shownMask and Pad.SELECT != 0, context.getString(R.string.pad_label_select))
        drawPill(canvas, l.start, shownMask and Pad.START != 0, context.getString(R.string.pad_label_start))
        canvas.restoreToCount(saved)
    }

    private fun idleFill() = if (highContrast) Color.BLACK else 0x55FFFFFF
    private fun pressedFill() = if (highContrast) Color.YELLOW else 0xE6FFFFFF.toInt()
    private fun outline() = if (highContrast) Color.WHITE else 0xCCFFFFFF.toInt()
    private fun labelColor(pressed: Boolean) = when {
        pressed -> Color.BLACK
        else -> Color.WHITE
    }

    /** Pressed controls get a thicker outline too, so state never rests on colour alone. */
    private fun strokeWidth(pressed: Boolean): Float {
        val base = if (highContrast) 4f else 2f
        return (if (pressed) base + 2f else base) * density
    }

    private fun drawRound(canvas: Canvas, c: Circle, pressed: Boolean, label: String) {
        fill.color = if (pressed) pressedFill() else idleFill()
        canvas.drawCircle(c.cx, c.cy, c.r, fill)
        stroke.color = outline()
        stroke.strokeWidth = strokeWidth(pressed)
        canvas.drawCircle(c.cx, c.cy, c.r, stroke)
        text.color = labelColor(pressed)
        text.textSize = c.r * 0.9f
        canvas.drawText(label, c.cx, c.cy - (text.descent() + text.ascent()) / 2f, text)
    }

    private fun drawPill(canvas: Canvas, b: Box, pressed: Boolean, label: String) {
        rectF.set(b.left, b.top, b.right, b.bottom)
        val r = b.height / 2f
        fill.color = if (pressed) pressedFill() else idleFill()
        canvas.drawRoundRect(rectF, r, r, fill)
        stroke.color = outline()
        stroke.strokeWidth = strokeWidth(pressed)
        canvas.drawRoundRect(rectF, r, r, stroke)
        text.color = labelColor(pressed)
        text.textSize = b.height * 0.45f
        canvas.drawText(label, b.cx, b.cy - (text.descent() + text.ascent()) / 2f, text)
    }

    private fun drawDpad(canvas: Canvas, c: Circle, mask: Int) {
        // Faint disc: the area that steers.
        fill.color = if (highContrast) 0x99000000.toInt() else 0x22FFFFFF
        canvas.drawCircle(c.cx, c.cy, c.r, fill)
        stroke.color = outline()
        stroke.strokeWidth = (if (highContrast) 3f else 1.5f) * density
        canvas.drawCircle(c.cx, c.cy, c.r, stroke)

        val arm = c.r * 0.34f // half-width of each arm
        val reach = c.r * 0.9f
        drawArm(canvas, c, Pad.UP, mask, c.cx - arm, c.cy - reach, c.cx + arm, c.cy)
        drawArm(canvas, c, Pad.DOWN, mask, c.cx - arm, c.cy, c.cx + arm, c.cy + reach)
        drawArm(canvas, c, Pad.LEFT, mask, c.cx - reach, c.cy - arm, c.cx, c.cy + arm)
        drawArm(canvas, c, Pad.RIGHT, mask, c.cx, c.cy - arm, c.cx + reach, c.cy + arm)
        // Centre square over the arm seams.
        fill.color = idleFill()
        canvas.drawRect(c.cx - arm, c.cy - arm, c.cx + arm, c.cy + arm, fill)
        // Arrow heads, so direction reads without colour.
        val t = arm * 0.7f
        text.color = Color.WHITE
        arrow(canvas, c.cx, c.cy - reach * 0.62f, t, 0, mask and Pad.UP != 0)
        arrow(canvas, c.cx, c.cy + reach * 0.62f, t, 2, mask and Pad.DOWN != 0)
        arrow(canvas, c.cx - reach * 0.62f, c.cy, t, 3, mask and Pad.LEFT != 0)
        arrow(canvas, c.cx + reach * 0.62f, c.cy, t, 1, mask and Pad.RIGHT != 0)
    }

    private fun drawArm(canvas: Canvas, c: Circle, bit: Int, mask: Int, l: Float, t: Float, r: Float, b: Float) {
        val pressed = mask and bit != 0
        rectF.set(l, t, r, b)
        val rr = c.r * 0.08f
        fill.color = if (pressed) pressedFill() else idleFill()
        canvas.drawRoundRect(rectF, rr, rr, fill)
        stroke.color = outline()
        stroke.strokeWidth = strokeWidth(pressed)
        canvas.drawRoundRect(rectF, rr, rr, stroke)
    }

    /** Triangle pointing up (0), right (1), down (2) or left (3). */
    private fun arrow(canvas: Canvas, x: Float, y: Float, s: Float, dir: Int, pressed: Boolean) {
        path.reset()
        when (dir) {
            0 -> { path.moveTo(x, y - s); path.lineTo(x + s, y + s * 0.6f); path.lineTo(x - s, y + s * 0.6f) }
            1 -> { path.moveTo(x + s, y); path.lineTo(x - s * 0.6f, y + s); path.lineTo(x - s * 0.6f, y - s) }
            2 -> { path.moveTo(x, y + s); path.lineTo(x - s, y - s * 0.6f); path.lineTo(x + s, y - s * 0.6f) }
            else -> { path.moveTo(x - s, y); path.lineTo(x + s * 0.6f, y - s); path.lineTo(x + s * 0.6f, y + s) }
        }
        path.close()
        fill.color = labelColor(pressed)
        canvas.drawPath(path, fill)
    }

    // --- ExploreByTouchHelper ----------------------------------------------------

    private inner class PadAccessibility : ExploreByTouchHelper(this@TouchPadView) {
        private val ids = listOf(V_UP, V_DOWN, V_LEFT, V_RIGHT, V_SELECT, V_START, V_B, V_A)

        override fun getVirtualViewAt(x: Float, y: Float): Int {
            val l = layoutInfo ?: return INVALID_ID
            if (visibility != VISIBLE) return INVALID_ID
            return when (l.hit(x, y)) {
                PadControl.A -> V_A
                PadControl.B -> V_B
                PadControl.SELECT -> V_SELECT
                PadControl.START -> V_START
                PadControl.DPAD -> {
                    // Four-way for exploring: the nearest arm.
                    val dx = x - l.dpad.cx
                    val dy = y - l.dpad.cy
                    if (abs(dx) > abs(dy)) (if (dx < 0) V_LEFT else V_RIGHT) else (if (dy < 0) V_UP else V_DOWN)
                }
                PadControl.NONE -> INVALID_ID
            }
        }

        override fun getVisibleVirtualViews(virtualViewIds: MutableList<Int>) {
            if (layoutInfo != null) virtualViewIds.addAll(ids)
        }

        override fun onPopulateNodeForVirtualView(virtualViewId: Int, node: AccessibilityNodeInfoCompat) {
            node.contentDescription = context.getString(labelOf(virtualViewId))
            node.className = "android.widget.Button"
            node.isClickable = true
            node.isFocusable = true
            node.addAction(AccessibilityNodeInfoCompat.AccessibilityActionCompat.ACTION_CLICK)
            @Suppress("DEPRECATION") // ExploreByTouchHelper still requires parent-relative bounds
            node.setBoundsInParent(boundsOf(virtualViewId))
        }

        override fun onPerformActionForVirtualView(virtualViewId: Int, action: Int, arguments: Bundle?): Boolean {
            if (action != AccessibilityNodeInfoCompat.ACTION_CLICK) return false
            pulse(bitOf(virtualViewId))
            invalidateVirtualView(virtualViewId)
            sendEventForVirtualView(virtualViewId, android.view.accessibility.AccessibilityEvent.TYPE_VIEW_CLICKED)
            return true
        }

        private fun boundsOf(id: Int): Rect {
            val l = layoutInfo ?: return Rect(0, 0, 1, 1)
            val min = l.minTarget
            val d = l.dpad
            val half = d.r / 2f
            val box = when (id) {
                V_UP -> Box(d.cx - half, d.cy - d.r, d.cx + half, d.cy - d.r * 0.15f)
                V_DOWN -> Box(d.cx - half, d.cy + d.r * 0.15f, d.cx + half, d.cy + d.r)
                V_LEFT -> Box(d.cx - d.r, d.cy - half, d.cx - d.r * 0.15f, d.cy + half)
                V_RIGHT -> Box(d.cx + d.r * 0.15f, d.cy - half, d.cx + d.r, d.cy + half)
                V_SELECT -> l.select
                V_START -> l.start
                V_B -> Box(l.b.cx - l.b.r, l.b.cy - l.b.r, l.b.cx + l.b.r, l.b.cy + l.b.r)
                else -> Box(l.a.cx - l.a.r, l.a.cy - l.a.r, l.a.cx + l.a.r, l.a.cy + l.a.r)
            }.atLeast(min, min)
            return Rect(box.left.roundToInt(), box.top.roundToInt(), box.right.roundToInt(), box.bottom.roundToInt())
        }
    }

    companion object {
        const val PULSE_MS = 120L

        private const val V_UP = 0
        private const val V_DOWN = 1
        private const val V_LEFT = 2
        private const val V_RIGHT = 3
        private const val V_SELECT = 4
        private const val V_START = 5
        private const val V_B = 6
        private const val V_A = 7

        private fun labelOf(id: Int) = when (id) {
            V_UP -> R.string.pad_up
            V_DOWN -> R.string.pad_down
            V_LEFT -> R.string.pad_left
            V_RIGHT -> R.string.pad_right
            V_SELECT -> R.string.pad_select
            V_START -> R.string.pad_start
            V_B -> R.string.pad_b
            else -> R.string.pad_a
        }

        private fun bitOf(id: Int) = when (id) {
            V_UP -> Pad.UP
            V_DOWN -> Pad.DOWN
            V_LEFT -> Pad.LEFT
            V_RIGHT -> Pad.RIGHT
            V_SELECT -> Pad.SELECT
            V_START -> Pad.START
            V_B -> Pad.B
            else -> Pad.A
        }
    }
}
