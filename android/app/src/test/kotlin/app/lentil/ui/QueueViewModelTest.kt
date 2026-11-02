package app.lentil.ui

import app.lentil.data.Finding
import app.lentil.data.Identity
import app.lentil.data.Intent
import app.lentil.data.IntentOutcome
import app.lentil.data.IntentState
import app.lentil.data.LedgerError
import app.lentil.data.StoredConflict
import app.lentil.data.StoredIntent
import app.lentil.data.usableOn
import java.time.Clock
import java.time.Instant
import java.time.ZoneOffset
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class QueueViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val grocery = entry("giro-3f9a1c2e7b4d")
    private val older = entry("giro-0a1b2c3d4e5f", date = "2026-10-01")
    private val ledger = FakeLedger(queue = listOf(grocery, older))
    private val store = FakeIntentStore()
    private val settings = FakeSettings()
    private val secrets = FakeSecrets()
    private var keys = 0

    private fun viewModel(deviceId: String? = "k7p2q9xd") =
        QueueViewModel(
            ledger = ledger,
            intents = store,
            settings = settings,
            secrets = secrets,
            identity = { deviceId?.let { Identity("lentil-android/0.1.0", it) } },
            clock = Clock.fixed(Instant.parse("2026-10-30T12:00:00Z"), ZoneOffset.UTC),
            newKey = { "key-${++keys}" },
        )

    @Test
    fun `start loads the queue and syncs`() =
        runTest(main.dispatcher) {
            val vm = viewModel()
            advanceUntilIdle()

            val state = vm.state.value
            assertEquals(listOf(grocery, older), state.items)
            assertEquals(SyncStatus.UpToDate, state.sync)
            assertEquals(1, ledger.syncs.size)
            val call = ledger.syncs.single()
            assertEquals("lentil-android/0.1.0", call.identity.client)
            assertEquals("k7p2q9xd", call.identity.device)
            assertEquals("Alex Example", call.author.name)
            assertEquals("alex", call.username)
            assertEquals("secret-token", call.token)
        }

    @Test
    fun `confirm records an intent, persists it and hides the transaction`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.confirm(grocery)
            advanceUntilIdle()

            val pending = listOf(StoredIntent("key-1", Intent.Confirm(grocery.id!!), grocery.label))
            assertEquals(pending, store.state.pending)
            assertEquals(pending, ledger.loads.last())
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(1, vm.state.value.pendingCount)
        }

    @Test
    fun `dismiss records an intent`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.dismiss(older)
            advanceUntilIdle()

            assertEquals(listOf(Intent.Dismiss(older.id!!)), store.state.pending.map { it.intent })
            assertEquals(listOf(grocery), vm.state.value.items)
        }

    @Test
    fun `changing the account uses the counter posting, confirms and remembers the account`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Groceries")
            assertEquals(listOf(older), vm.state.value.items)
            advanceUntilIdle()

            assertEquals(
                listOf(
                    Intent.SetAccount(
                        id = grocery.id!!,
                        posting = 1,
                        old = "Expenses:Uncategorized",
                        new = "Expenses:Groceries",
                    ),
                    Intent.Confirm(grocery.id!!),
                ),
                store.state.pending.map { it.intent },
            )
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(2, vm.state.value.pendingCount)
            assertEquals(listOf("Expenses:Groceries"), vm.state.value.recentAccounts)
        }

    @Test
    fun `choosing the same account records nothing`() =
        runTest(main.dispatcher) {
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Uncategorized")
            assertEquals(listOf(grocery, older), vm.state.value.items)
            advanceUntilIdle()

            assertEquals(emptyList<StoredIntent>(), store.state.pending)
        }

    @Test
    fun `a new account opens on the default date before it is set and confirmed`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.openAccount(grocery, "Expenses:Garden")
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(
                listOf("Expenses:Garden", "Expenses:Groceries", "Expenses:Uncategorized"),
                vm.state.value.accounts.usableOn("2026-10-18"),
            )
            advanceUntilIdle()

            assertEquals(
                listOf(
                    Intent.OpenAccount("Expenses:Garden", "2026-10-18"),
                    Intent.SetAccount(grocery.id!!, 1, "Expenses:Uncategorized", "Expenses:Garden"),
                    Intent.Confirm(grocery.id!!),
                ),
                store.state.pending.map { it.intent },
            )
            assertEquals(listOf(older), vm.state.value.items)
        }

    @Test
    fun `without a default date a new account opens today`() =
        runTest(main.dispatcher) {
            ledger.defaultOpenDate = null
            val vm = viewModel()
            advanceUntilIdle()
            ledger.syncError = LedgerError.Network

            vm.openAccount(grocery, "Expenses:Garden")
            advanceUntilIdle()

            assertEquals(
                Intent.OpenAccount("Expenses:Garden", "2026-10-30"),
                store.state.pending.first().intent,
            )
        }

    @Test
    fun `a split or a transaction without ID is not edited`() =
        runTest(main.dispatcher) {
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(entry("giro-111111111111", counter = null, posting = null), "X:Y")
            vm.confirm(entry(null))
            advanceUntilIdle()

            assertEquals(emptyList<StoredIntent>(), store.state.pending)
        }

    @Test
    fun `sync removes applied intents and moves conflicts to the list`() =
        runTest(main.dispatcher) {
            val confirm = StoredIntent("a", Intent.Confirm("giro-3f9a1c2e7b4d"), "label a")
            val done = StoredIntent("b", Intent.Dismiss("giro-0a1b2c3d4e5f"), "label b")
            val conflicting =
                StoredIntent(
                    "c",
                    Intent.SetAccount(
                        "giro-222222222222",
                        1,
                        "Expenses:Uncategorized",
                        "Expenses:Food",
                    ),
                    "label c",
                )
            store.state = IntentState(pending = listOf(confirm, done, conflicting))
            ledger.outcomes = {
                mapOf(
                    "a" to IntentOutcome.Applied,
                    "b" to IntentOutcome.AlreadyApplied,
                    "c" to IntentOutcome.Conflict("AccountChanged", "Expenses:Travel"),
                )
            }

            val vm = viewModel()
            advanceUntilIdle()

            assertEquals(listOf(confirm, done, conflicting), ledger.syncs.single().pending)
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            val conflict = StoredConflict(conflicting, "AccountChanged", "Expenses:Travel")
            assertEquals(listOf(conflict), store.state.conflicts)
            assertEquals(listOf(conflict), vm.state.value.conflicts)
            assertEquals(0, vm.state.value.pendingCount)
        }

    @Test
    fun `a failed sync keeps every intent`() =
        runTest(main.dispatcher) {
            val pending = listOf(StoredIntent("a", Intent.Confirm("giro-3f9a1c2e7b4d"), "label"))
            store.state = IntentState(pending = pending)
            ledger.syncError = LedgerError.Network

            val vm = viewModel()
            advanceUntilIdle()

            assertEquals(SyncStatus.Offline, vm.state.value.sync)
            assertEquals(pending, store.state.pending)
            assertEquals(1, vm.state.value.pendingCount)
        }

    @Test
    fun `sync errors become user-facing states`() =
        runTest(main.dispatcher) {
            val findings = listOf(Finding("AccountNotOpen", true, null, null, mapOf()))
            val cases =
                mapOf(
                    LedgerError.Auth to SyncStatus.SignInFailed,
                    LedgerError.NotFound to SyncStatus.RemoteMissing,
                    LedgerError.TooManyAttempts to SyncStatus.Busy,
                    LedgerError.Blocked(findings) to SyncStatus.Blocked(findings),
                    LedgerError.ReadOnly(findings) to SyncStatus.ReadOnly(findings),
                    LedgerError.Other("boom") to SyncStatus.Failed("boom"),
                )
            val vm = viewModel()
            advanceUntilIdle()
            for ((error, status) in cases) {
                ledger.syncError = error
                vm.sync()
                advanceUntilIdle()
                assertEquals(status, vm.state.value.sync)
            }
        }

    @Test
    fun `without a token or device ID nothing is sent`() =
        runTest(main.dispatcher) {
            secrets.clear()
            val vm = viewModel()
            advanceUntilIdle()
            assertEquals(SyncStatus.SignInFailed, vm.state.value.sync)

            secrets.saveToken("secret-token")
            val noDevice = viewModel(deviceId = null)
            advanceUntilIdle()
            assertTrue(noDevice.state.value.sync is SyncStatus.Failed)
            assertEquals(0, ledger.syncs.size)
        }

    @Test
    fun `intents recorded during a sync stay pending`() =
        runTest(main.dispatcher) {
            val first = StoredIntent("a", Intent.Confirm(older.id!!), "label")
            store.state = IntentState(pending = listOf(first))
            val gate = CompletableDeferred<Unit>()
            ledger.syncGate = gate

            val vm = viewModel()
            advanceUntilIdle()
            vm.confirm(grocery)
            gate.complete(Unit)
            advanceUntilIdle()

            assertEquals(
                listOf(StoredIntent("key-1", Intent.Confirm(grocery.id!!), grocery.label)),
                store.state.pending,
            )
        }

    @Test
    fun `a conflict stays until the user discards it`() =
        runTest(main.dispatcher) {
            val intent = StoredIntent("c", Intent.Dismiss("giro-3f9a1c2e7b4d"), "label")
            store.state =
                IntentState(conflicts = listOf(StoredConflict(intent, "TransactionNotFound")))
            val vm = viewModel()
            advanceUntilIdle()
            assertEquals(1, vm.state.value.conflicts.size)

            vm.discardConflict("c")
            advanceUntilIdle()

            assertEquals(emptyList<StoredConflict>(), vm.state.value.conflicts)
            assertEquals(emptyList<StoredConflict>(), store.state.conflicts)
        }

    @Test
    fun `a read-only ledger accepts no edits`() =
        runTest(main.dispatcher) {
            ledger.readOnly = listOf(Finding("OutsideProfile", true, "main.beancount", 3, mapOf()))
            val vm = viewModel()
            advanceUntilIdle()

            vm.confirm(grocery)
            vm.dismiss(older)
            advanceUntilIdle()

            assertEquals(false, vm.state.value.writable)
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertEquals(listOf(grocery, older), vm.state.value.items)
        }

    @Test
    fun `a ledger without valid configuration shows its findings`() =
        runTest(main.dispatcher) {
            val findings = listOf(Finding("ConfigInvalid", true, "lentil.toml", null, mapOf()))
            ledger.loadError = LedgerError.ReadOnly(findings)
            val vm = viewModel()
            advanceUntilIdle()

            assertEquals(findings, vm.state.value.readOnly)
            assertEquals(null, vm.state.value.loadError)
            assertEquals(false, vm.state.value.loading)
        }
}
