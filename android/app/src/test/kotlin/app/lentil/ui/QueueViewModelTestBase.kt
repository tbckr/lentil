package app.lentil.ui

import app.lentil.data.Identity
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import org.junit.Rule

/** A [QueueViewModel] on fakes, with a queue of two transactions. */
abstract class QueueViewModelTestBase {
    @get:Rule val main = MainDispatcherRule()

    protected val grocery = entry("giro-3f9a1c2e7b4d")
    protected val older = entry("giro-0a1b2c3d4e5f", date = "2026-10-01")
    protected val ledger = FakeLedger(queue = listOf(grocery, older))
    protected val store = FakeIntentStore()
    protected val settings = FakeSettings()
    protected val secrets = FakeSecrets()
    private var keys = 0

    protected fun viewModel(deviceId: String? = "k7p2q9xd") =
        QueueViewModel(
            ledger = ledger,
            intents = store,
            settings = settings,
            secrets = secrets,
            identity = { deviceId?.let { Identity("lentil-android/0.1.0", it) } },
            clock = Clock.fixed(Instant.parse("2026-10-30T12:00:00Z"), ZoneOffset.UTC),
            newKey = { "key-${++keys}" },
        )
}
