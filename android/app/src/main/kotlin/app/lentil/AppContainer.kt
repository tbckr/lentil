package app.lentil

import android.content.Context
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import app.lentil.data.FfiLedgerRepository
import app.lentil.data.Identity
import app.lentil.data.IntentStore
import app.lentil.data.JsonIntentStore
import app.lentil.data.KeystoreSecretStore
import app.lentil.data.LedgerRepository
import app.lentil.data.PreferencesSettingsStore
import app.lentil.data.SecretStore
import app.lentil.data.SettingsStore
import app.lentil.data.SystemCaBundle
import app.lentil.ui.ConnectViewModel
import app.lentil.ui.QueueViewModel
import java.io.File
import java.time.Clock

/** Wires the app by hand (CONTRIBUTING: Architecture); one instance per process. */
class AppContainer(context: Context) {
    private val storage: File = context.noBackupFilesDir

    val settings: SettingsStore =
        PreferencesSettingsStore(context.getSharedPreferences("settings", Context.MODE_PRIVATE))
    val secrets: SecretStore =
        KeystoreSecretStore(context.getSharedPreferences("secrets", Context.MODE_PRIVATE))
    val intents: IntentStore = JsonIntentStore(File(storage, "intents.json"))
    val ledger: LedgerRepository =
        FfiLedgerRepository(File(storage, "ledger.git"), SystemCaBundle(File(storage, "ca.pem")))
    private val clock: Clock = Clock.systemDefaultZone()

    /** `Lentil-Client` of every commit (spec section 12). */
    private val client = "lentil-android/${BuildConfig.VERSION_NAME}"

    private fun identity(): Identity? = deviceId()?.let { Identity(client = client, device = it) }

    fun isConnected(): Boolean =
        ledger.hasClone() && settings.connection != null && secrets.hasToken()

    /** The device ID, created once on first start (spec section 9); `null` if the core failed. */
    @Suppress("TooGenericExceptionCaught", "SwallowedException")
    fun deviceId(): String? =
        settings.deviceId
            ?: try {
                ledger.newDeviceId().also { settings.deviceId = it }
            } catch (e: Exception) {
                // UniFFI reports a failure of the core as an exception. Without an ID the sync
                // reports the problem, and the next sync tries again.
                null
            }

    val connectViewModels = viewModelFactory {
        initializer { ConnectViewModel(ledger, settings, secrets) }
    }

    val queueViewModels = viewModelFactory {
        initializer {
            QueueViewModel(
                ledger = ledger,
                intents = intents,
                settings = settings,
                secrets = secrets,
                identity = ::identity,
                clock = clock,
            )
        }
    }
}
