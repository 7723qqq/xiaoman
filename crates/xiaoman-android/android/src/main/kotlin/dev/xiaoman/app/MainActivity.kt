package dev.xiaoman.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import dev.xiaoman.app.ui.HostUi

/**
 * The app's single activity: the five screens over the Rust session.
 *
 * The task, the operator loop, the sentinel and every device decision live in
 * `libxiaoman_android.so`; this activity only renders what [SessionStore] polls and forwards the
 * user's input.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent { HostUi() }
    }
}
