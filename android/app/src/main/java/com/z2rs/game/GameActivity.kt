package com.z2rs.game

import android.hardware.input.InputManager
import android.os.Build
import android.os.Bundle
import android.os.Process
import android.view.Gravity
import android.view.InputDevice
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.FrameLayout
import android.widget.ImageButton
import android.widget.Toast
import androidx.activity.addCallback
import androidx.appcompat.app.AlertDialog
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import com.google.android.material.dialog.MaterialAlertDialogBuilder
import kotlin.math.roundToInt

/**
 * Hosts the Rust game (android_main in libz2rs_android.so, loaded by the
 * GameActivity base class from the android.app.lib_name meta-data) and adds
 * the Android-side controls on top of it.
 *
 * View tree: GameActivity.onCreateSurfaceView() makes a FrameLayout holding
 * its InputEnabledSurfaceView and sets it as the content view. After
 * super.onCreate() we add a second full-screen FrameLayout with
 * addContentView(), so it is a sibling drawn above the game's frame inside
 * android.R.id.content. It holds the [TouchPadView] and the pause button.
 * Touches that miss every control are not consumed and fall through to the
 * surface; keys and joystick motion are intercepted at the activity level.
 *
 * In portrait with the pad shown, the surface view itself is resized to the
 * area above the control strip, so the letterboxed game sits above the
 * controls rather than under them. In landscape it fills the screen (minus
 * any display cutout) and the pad floats over it, translucent.
 */
class GameActivity : com.google.androidgamesdk.GameActivity(), InputManager.InputDeviceListener {

    private lateinit var settings: Settings
    private lateinit var overlay: FrameLayout
    private lateinit var padView: TouchPadView
    private lateinit var pauseButton: ImageButton
    private lateinit var inputManager: InputManager

    private val controllers = ControllerRegistry { player, mask -> Native.setHardwarePad(player, mask) }

    /** Session override from the pause menu; null = follow the settings. */
    private var padOverride: Boolean? = null
    private var menu: AlertDialog? = null
    private var cutout = Insets4.NONE
    private var lastFrameKey = ""
    private val controllerNames = HashMap<Int, String>()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        settings = Settings(this)
        setUpWindow()

        val d = resources.displayMetrics.density
        overlay = FrameLayout(this).apply {
            // The overlay must never take key focus from the game surface
            // (keyboard input goes to winit through it).
            isFocusable = false
            descendantFocusability = ViewGroup.FOCUS_BLOCK_DESCENDANTS
        }
        padView = TouchPadView(this).apply { configure(settings) }
        overlay.addView(padView, FrameLayout.LayoutParams(MATCH, MATCH))

        pauseButton = ImageButton(this).apply {
            setImageResource(R.drawable.ic_pause)
            setBackgroundResource(R.drawable.bg_pause_button)
            contentDescription = getString(R.string.pause_button_desc)
            isFocusable = false
            isFocusableInTouchMode = false
            minimumWidth = (48 * d).roundToInt()
            minimumHeight = (48 * d).roundToInt()
            alpha = 0.8f
            setOnClickListener { openMenu() }
        }
        val size = (48 * d).roundToInt()
        overlay.addView(pauseButton, FrameLayout.LayoutParams(size, size, Gravity.TOP or Gravity.END))

        addContentView(overlay, ViewGroup.LayoutParams(MATCH, MATCH))

        ViewCompat.setOnApplyWindowInsetsListener(overlay) { _, insets ->
            val c = insets.getInsets(WindowInsetsCompat.Type.displayCutout())
            cutout = Insets4(c.left.toFloat(), c.top.toFloat(), c.right.toFloat(), c.bottom.toFloat())
            relayout(force = true)
            insets // not consumed: GameActivity's own listener sees the same insets
        }
        overlay.addOnLayoutChangeListener { _, _, _, _, _, _, _, _, _ -> relayout(force = false) }

        onBackPressedDispatcher.addCallback(this) { openMenu() }

        inputManager = getSystemService(INPUT_SERVICE) as InputManager
        inputManager.registerInputDeviceListener(this, null)
        for (id in InputDevice.getDeviceIds()) {
            if (isController(InputDevice.getDevice(id))) controllers.connect(id)
        }
        updatePadVisibility()
    }

    private fun setUpWindow() {
        if (settings.keepScreenOn) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes = window.attributes.apply {
                layoutInDisplayCutoutMode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                } else {
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
            }
        }
        WindowCompat.setDecorFitsSystemWindows(window, false)
        hideSystemBars()
    }

    private fun hideSystemBars() {
        WindowCompat.getInsetsController(window, window.decorView).apply {
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
            hide(WindowInsetsCompat.Type.systemBars())
        }
    }

    // --- layout --------------------------------------------------------------

    private fun relayout(force: Boolean) {
        val w = overlay.width
        val h = overlay.height
        if (w == 0 || h == 0) return
        val portrait = h > w
        val padShown = padView.visibility == View.VISIBLE
        val key = "$w:$h:$padShown:$cutout:${settings.padSize}"
        if (!force && key == lastFrameKey) return
        lastFrameKey = key

        padView.setFrame(portrait, cutout)

        // Game surface: out of the cutout, and above the control strip in portrait.
        val surface = mSurfaceView ?: return
        val lp = (surface.layoutParams as? FrameLayout.LayoutParams)
            ?: FrameLayout.LayoutParams(MATCH, MATCH)
        val newLp = FrameLayout.LayoutParams(lp)
        newLp.gravity = Gravity.TOP or Gravity.CENTER_HORIZONTAL
        newLp.leftMargin = cutout.left.roundToInt()
        newLp.rightMargin = cutout.right.roundToInt()
        newLp.topMargin = cutout.top.roundToInt()
        if (portrait && padShown) {
            val area = PadGeometry.portraitAreaHeight(h.toFloat(), resources.displayMetrics.density, settings.padSize)
            val areaTop = h - cutout.bottom - area
            newLp.width = MATCH
            newLp.height = (areaTop - cutout.top).roundToInt().coerceAtLeast(1)
            newLp.bottomMargin = 0
        } else {
            newLp.width = MATCH
            newLp.height = MATCH
            newLp.bottomMargin = cutout.bottom.roundToInt()
        }
        if (!sameLayout(lp, newLp)) surface.layoutParams = newLp

        val m = (8 * resources.displayMetrics.density).roundToInt()
        (pauseButton.layoutParams as FrameLayout.LayoutParams).let { p ->
            val top = cutout.top.roundToInt() + m
            val end = cutout.right.roundToInt() + m
            if (p.topMargin != top || p.marginEnd != end) {
                p.topMargin = top
                p.marginEnd = end
                pauseButton.layoutParams = p
            }
        }
    }

    private fun sameLayout(a: FrameLayout.LayoutParams, b: FrameLayout.LayoutParams) =
        a.width == b.width && a.height == b.height && a.gravity == b.gravity &&
            a.leftMargin == b.leftMargin && a.topMargin == b.topMargin &&
            a.rightMargin == b.rightMargin && a.bottomMargin == b.bottomMargin

    private fun updatePadVisibility() {
        val auto = settings.touchEnabled &&
            !(settings.hideTouchWithController && controllers.count > 0)
        val shown = padOverride ?: auto
        val v = if (shown) View.VISIBLE else View.GONE
        if (padView.visibility != v) {
            padView.visibility = v
            relayout(force = true)
        }
    }

    // --- pause menu ----------------------------------------------------------

    private fun openMenu() {
        if (menu?.isShowing == true || isFinishing) return
        Native.setPaused(true)
        releaseInputs()
        val padShown = padView.visibility == View.VISIBLE
        val items = arrayOf<CharSequence>(
            getString(R.string.menu_resume),
            getString(R.string.menu_save_state),
            getString(R.string.menu_load_state),
            getString(if (padShown) R.string.menu_hide_pad else R.string.menu_show_pad),
            getString(R.string.menu_quit),
        )
        menu = MaterialAlertDialogBuilder(this)
            .setTitle(R.string.menu_title)
            .setItems(items) { _, which ->
                when (which) {
                    0 -> resumeGame()
                    1 -> {
                        resumeGame()
                        Native.requestSaveState(SLOT)
                        Toast.makeText(this, R.string.state_saving, Toast.LENGTH_SHORT).show()
                    }
                    2 -> {
                        resumeGame()
                        Native.requestLoadState(SLOT)
                        Toast.makeText(this, R.string.state_loading, Toast.LENGTH_SHORT).show()
                    }
                    3 -> {
                        padOverride = !padShown
                        updatePadVisibility()
                        resumeGame()
                    }
                    4 -> quitToLauncher()
                }
            }
            .setOnCancelListener { resumeGame() }
            .show()
    }

    /** Unpauses now; onWindowFocusChanged(true) repeats it once the dialog has gone. */
    private fun resumeGame() {
        Native.setPaused(false)
        hideSystemBars()
    }

    private fun quitToLauncher() {
        releaseInputs()
        finish()
    }

    // --- hardware controllers -----------------------------------------------

    override fun dispatchKeyEvent(event: KeyEvent): Boolean {
        val code = event.keyCode
        if (code == KeyEvent.KEYCODE_BACK) {
            // Back (key, button or gesture) opens the pause menu.
            if (event.action == KeyEvent.ACTION_UP && !event.isCanceled) openMenu()
            return true
        }
        if (isControllerKey(event)) {
            if (GamepadMapping.isMenuKey(code)) {
                if (event.action == KeyEvent.ACTION_UP) openMenu()
                return true
            }
            val bits = GamepadMapping.keyBits(code)
            if (bits != 0) {
                when (event.action) {
                    KeyEvent.ACTION_DOWN -> if (event.repeatCount == 0) controllers.onKey(event.deviceId, bits, true)
                    KeyEvent.ACTION_UP -> controllers.onKey(event.deviceId, bits, false)
                }
                updatePadVisibility()
                return true // winit must not also see it
            }
            // Unmapped controller buttons (shoulders, sticks...) do nothing.
            if (KeyEvent.isGamepadButton(code)) return true
        }
        // Keyboards and everything else: GameActivity -> native -> winit.
        return super.dispatchKeyEvent(event)
    }

    override fun dispatchGenericMotionEvent(event: MotionEvent): Boolean {
        if (event.isFromSource(InputDevice.SOURCE_JOYSTICK) && event.actionMasked == MotionEvent.ACTION_MOVE) {
            val bits = GamepadMapping.motionBits(
                event.getAxisValue(MotionEvent.AXIS_X),
                event.getAxisValue(MotionEvent.AXIS_Y),
                event.getAxisValue(MotionEvent.AXIS_HAT_X),
                event.getAxisValue(MotionEvent.AXIS_HAT_Y),
            )
            controllers.onAxes(event.deviceId, bits)
            updatePadVisibility()
            return true
        }
        return super.dispatchGenericMotionEvent(event)
    }

    private fun isControllerKey(event: KeyEvent): Boolean {
        val src = event.source
        if (src and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD) return true
        if (src and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK) return true
        if (KeyEvent.isGamepadButton(event.keyCode)) return true
        if (src and InputDevice.SOURCE_DPAD == InputDevice.SOURCE_DPAD) {
            // Keyboards report their arrow keys as DPAD too; those stay with winit.
            val dev = event.device ?: return false
            return dev.keyboardType != InputDevice.KEYBOARD_TYPE_ALPHABETIC && !dev.isVirtual
        }
        return false
    }

    private fun isController(dev: InputDevice?): Boolean {
        if (dev == null || dev.isVirtual) return false
        val s = dev.sources
        return s and InputDevice.SOURCE_GAMEPAD == InputDevice.SOURCE_GAMEPAD ||
            s and InputDevice.SOURCE_JOYSTICK == InputDevice.SOURCE_JOYSTICK
    }

    override fun onInputDeviceAdded(deviceId: Int) {
        val dev = InputDevice.getDevice(deviceId)
        if (isController(dev) && controllers.connect(deviceId)) {
            val name = dev?.name.orEmpty()
            controllerNames[deviceId] = name
            controllers.playerOf(deviceId)?.let { p ->
                overlay.announceForAccessibility(getString(R.string.controller_connected, name, p + 1))
            }
        }
        updatePadVisibility()
    }

    override fun onInputDeviceRemoved(deviceId: Int) {
        if (deviceId in controllers.deviceIds()) {
            controllers.disconnect(deviceId) // zeroes its buttons
            val name = controllerNames.remove(deviceId).orEmpty()
            overlay.announceForAccessibility(getString(R.string.controller_disconnected, name))
        }
        updatePadVisibility()
    }

    override fun onInputDeviceChanged(deviceId: Int) {
        val dev = InputDevice.getDevice(deviceId)
        if (isController(dev)) controllers.connect(deviceId)
        else if (deviceId in controllers.deviceIds()) controllers.disconnect(deviceId)
        updatePadVisibility()
    }

    // --- lifecycle -------------------------------------------------------------

    private fun releaseInputs() {
        if (::padView.isInitialized) padView.releaseAll()
        controllers.releaseAll()
    }

    override fun onResume() {
        super.onResume()
        hideSystemBars()
    }

    override fun onPause() {
        releaseInputs()
        super.onPause()
    }

    override fun onWindowFocusChanged(hasFocus: Boolean) {
        super.onWindowFocusChanged(hasFocus)
        if (hasFocus) {
            hideSystemBars()
            // The Rust side pauses on focus loss and never resumes by itself.
            if (menu?.isShowing != true) Native.setPaused(false)
        } else {
            releaseInputs()
        }
    }

    override fun onDestroy() {
        if (::inputManager.isInitialized) inputManager.unregisterInputDeviceListener(this)
        menu?.dismiss()
        super.onDestroy()
        if (isFinishing) {
            // GameActivity.onDestroy has waited for android_main to return.
            // winit allows one event loop per process, so end this ":game"
            // process; the next Play starts a fresh one.
            Process.killProcess(Process.myPid())
        }
    }

    private companion object {
        const val MATCH = ViewGroup.LayoutParams.MATCH_PARENT
        const val SLOT = 1
    }
}
