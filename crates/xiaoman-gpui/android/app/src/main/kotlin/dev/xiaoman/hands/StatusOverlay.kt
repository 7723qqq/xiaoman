package dev.xiaoman.hands

import android.content.Context
import android.graphics.PixelFormat
import android.graphics.drawable.GradientDrawable
import android.os.Handler
import android.os.Looper
import android.text.TextUtils
import android.util.Log
import android.util.TypedValue
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import dev.xiaoman.app.R
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * The status pill of the original app: the current step plus a Stop button, in a
 * `TYPE_ACCESSIBILITY_OVERLAY` window. Only the connected accessibility service may add it, and
 * the window wraps its content, so touches outside the pill reach the app underneath.
 */
class StatusOverlay(private val context: Context) {

    var onStop: () -> Unit = {}
    var onOpen: () -> Unit = {}

    private val main = Handler(Looper.getMainLooper())
    private val windowManager = context.getSystemService(WindowManager::class.java)
    private var root: LinearLayout? = null
    private var textView: TextView? = null
    private var pendingText: CharSequence = ""

    val isShowing: Boolean get() = root?.isAttachedToWindow == true

    fun show() = onMain {
        if (root != null) return@onMain
        val view = buildView()
        try {
            windowManager.addView(view, layoutParams())
            root = view
        } catch (error: RuntimeException) {
            Log.w(TAG, "cannot show overlay: ${error.message}")
            root = null
            textView = null
        }
    }

    fun update(text: CharSequence) = onMain {
        pendingText = text
        textView?.text = text
    }

    fun hide() = onMain {
        val view = root ?: return@onMain
        root = null
        textView = null
        runCatching { windowManager.removeViewImmediate(view) }
    }

    /** Hides the pill and makes its window untouchable while a screenshot or gesture runs. */
    fun hideForCapture() {
        val done = CountDownLatch(1)
        main.post {
            val view = root
            if (view != null && view.visibility == View.VISIBLE) {
                val params = view.layoutParams as WindowManager.LayoutParams
                params.flags = params.flags or WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE
                runCatching { windowManager.updateViewLayout(view, params) }
                view.visibility = View.GONE
            }
            done.countDown()
        }
        // The frame and input window update have to land before the capture.
        done.await(120, TimeUnit.MILLISECONDS)
        Thread.sleep(60)
    }

    fun restoreAfterCapture() {
        main.post {
            val view = root ?: return@post
            val params = view.layoutParams as WindowManager.LayoutParams
            params.flags = params.flags and WindowManager.LayoutParams.FLAG_NOT_TOUCHABLE.inv()
            runCatching { windowManager.updateViewLayout(view, params) }
            view.visibility = View.VISIBLE
        }
    }

    private fun buildView(): LinearLayout {
        val pad = dp(10)
        val label = TextView(context).apply {
            setTextColor(0xFFFFFFFF.toInt())
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            maxLines = 2
            ellipsize = TextUtils.TruncateAt.END
            maxWidth = (context.resources.displayMetrics.widthPixels * 0.52f).toInt()
            text = pendingText
            minHeight = dp(48)
            gravity = Gravity.CENTER_VERTICAL
            contentDescription = "查看小满的任务"
            setOnClickListener { onOpen() }
            accessibilityLiveRegion = View.ACCESSIBILITY_LIVE_REGION_POLITE
        }
        val stop = Button(context).apply {
            setText(R.string.xiaoman_overlay_stop)
            contentDescription = context.getString(R.string.xiaoman_overlay_stop_description)
            minimumHeight = dp(48)
            minHeight = dp(48)
            setTextSize(TypedValue.COMPLEX_UNIT_SP, 12f)
            setOnClickListener { onStop() }
        }
        textView = label
        return LinearLayout(context).apply {
            orientation = LinearLayout.HORIZONTAL
            gravity = Gravity.CENTER_VERTICAL
            setPadding(pad * 2, pad / 2, pad / 2, pad / 2)
            background = GradientDrawable().apply {
                cornerRadius = dp(24).toFloat()
                setColor(0xF01E4635.toInt())
            }
            addView(
                label,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ).apply { marginEnd = pad },
            )
            addView(
                stop,
                LinearLayout.LayoutParams(
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                    LinearLayout.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
    }

    private fun layoutParams() = WindowManager.LayoutParams(
        WindowManager.LayoutParams.WRAP_CONTENT,
        WindowManager.LayoutParams.WRAP_CONTENT,
        WindowManager.LayoutParams.TYPE_ACCESSIBILITY_OVERLAY,
        WindowManager.LayoutParams.FLAG_NOT_FOCUSABLE or
            WindowManager.LayoutParams.FLAG_NOT_TOUCH_MODAL or
            WindowManager.LayoutParams.FLAG_LAYOUT_IN_SCREEN,
        PixelFormat.TRANSLUCENT,
    ).apply {
        gravity = Gravity.TOP or Gravity.CENTER_HORIZONTAL
        y = dp(36)
        title = "Xiaoman status"
    }

    private fun dp(value: Int): Int = (value * context.resources.displayMetrics.density).toInt()

    private fun onMain(block: () -> Unit) {
        if (Looper.myLooper() == Looper.getMainLooper()) block() else main.post(block)
    }

    private companion object {
        const val TAG = "Xiaoman"
    }
}

/** The connected service and the one pill, shared by the service and the notifier bridge. */
object OverlayHost {
    @Volatile
    var service: XiaomanAccessibilityService? = null

    @Volatile
    var overlay: StatusOverlay? = null
}
