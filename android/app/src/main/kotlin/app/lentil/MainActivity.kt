package app.lentil

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.compose.LifecycleEventEffect
import androidx.lifecycle.viewmodel.compose.viewModel
import app.lentil.ui.ConnectScreen
import app.lentil.ui.ConnectViewModel
import app.lentil.ui.LentilTheme
import app.lentil.ui.QueueScreen
import app.lentil.ui.QueueViewModel

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        val container = (application as LentilApp).container
        setContent { LentilTheme { Lentil(container) } }
    }
}

@Composable
private fun Lentil(container: AppContainer) {
    var connected by rememberSaveable { mutableStateOf(container.isConnected()) }
    // Each connection gets fresh view models.
    var session by rememberSaveable { mutableIntStateOf(0) }
    if (connected) {
        val queue: QueueViewModel =
            viewModel(key = "queue-$session", factory = container.queueViewModels)
        // Leaving the app ends a session; push what it recorded (spec section 11).
        LifecycleEventEffect(Lifecycle.Event.ON_STOP) { queue.syncIfPending() }
        // Changes saved on the device stay in the intent store and are replayed on the new clone.
        QueueScreen(
            queue,
            onChangeConnection = {
                session++
                connected = false
            },
        )
    } else {
        val connect: ConnectViewModel =
            viewModel(key = "connect-$session", factory = container.connectViewModels)
        val backToQueue = {
            session++
            connected = true
        }
        ConnectScreen(
            connect,
            onConnected = backToQueue,
            onCancel = backToQueue.takeIf { container.isConnected() },
        )
    }
}
