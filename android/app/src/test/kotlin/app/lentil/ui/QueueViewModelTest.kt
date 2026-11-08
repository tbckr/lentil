package app.lentil.ui

import app.lentil.data.Finding
import app.lentil.data.Intent
import app.lentil.data.IntentOutcome
import app.lentil.data.IntentState
import app.lentil.data.LedgerError
import app.lentil.data.QueueEntry
import app.lentil.data.StoredConflict
import app.lentil.data.StoredIntent
import app.lentil.data.TagChoice
import app.lentil.data.TagChoices
import app.lentil.data.TagSource
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class QueueViewModelTest : QueueViewModelTestBase() {
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
    fun `a split or a transaction without ID is not edited`() =
        runTest(main.dispatcher) {
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(entry("giro-111111111111", counter = null, posting = null), "X:Y")
            vm.confirm(entry(null))
            advanceUntilIdle()

            assertTrue(ledger.corrections.isEmpty())
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

    private val tagged = entry("giro-3f9a1c2e7b4d", tags = listOf("friends"))
    private val suggested = entry("giro-3f9a1c2e7b4d", suggested = "trip-lisbon")

    @Test
    fun `removing a tag keeps the transaction open`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.queue = listOf(tagged, older)
            val vm = viewModel()
            advanceUntilIdle()

            vm.removeTag(tagged, "friends")
            advanceUntilIdle()

            assertEquals(
                listOf(
                    StoredIntent("key-1", Intent.RemoveTag(tagged.id!!, "friends"), tagged.label)
                ),
                store.state.pending,
            )
            assertTrue(tagged in vm.state.value.items)
        }

    @Test
    fun `picking a tag records the core's intents in order and never confirms`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.picks = { id, tag ->
                listOf(Intent.RemoveTag(id, "recurring"), Intent.AddTag(id, tag))
            }
            val vm = viewModel()
            advanceUntilIdle()

            vm.pickTag(grocery, "irregular")
            advanceUntilIdle()

            assertEquals(listOf(grocery.id!! to "irregular"), ledger.picked)
            assertEquals(
                listOf(
                    Intent.RemoveTag(grocery.id!!, "recurring"),
                    Intent.AddTag(grocery.id!!, "irregular"),
                ),
                store.state.pending.map { it.intent },
            )
            assertTrue(grocery in vm.state.value.items)
            assertNull(vm.state.value.tagPicker)
        }

    @Test
    fun `the tag picker shows the core's choices`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.choices =
                TagChoices(
                    listOf(TagChoice("irregular", listOf("recurring"))),
                    listOf("share-"),
                    TagSource.Missing,
                )
            val vm = viewModel()
            advanceUntilIdle()

            vm.openTagPicker(grocery)
            advanceUntilIdle()
            assertEquals(TagPicker(grocery, ledger.choices), vm.state.value.tagPicker)

            vm.closeTagPicker()
            assertNull(vm.state.value.tagPicker)
        }

    @Test
    fun `accepting the suggestion picks the suggested tag`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.queue = listOf(suggested)
            val vm = viewModel()
            advanceUntilIdle()

            vm.acceptSuggestion(suggested)
            advanceUntilIdle()

            assertEquals(listOf(suggested.id!! to "trip-lisbon"), ledger.picked)
            assertEquals(
                listOf(Intent.AddTag(suggested.id!!, "trip-lisbon")),
                store.state.pending.map { it.intent },
            )
        }

    @Test
    fun `confirming never accepts the suggestion`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.queue = listOf(suggested)
            val vm = viewModel()
            advanceUntilIdle()

            vm.confirm(suggested)
            advanceUntilIdle()

            assertEquals(
                listOf(Intent.Confirm(suggested.id!!)),
                store.state.pending.map { it.intent },
            )
            assertTrue(ledger.picked.isEmpty())
        }

    @Test
    fun `a read-only ledger or a transaction without ID takes no tag edits`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val withoutId = entry(null, tags = listOf("friends"), suggested = "trip-lisbon")
            ledger.queue = listOf(suggested, withoutId)
            val vm = viewModel()
            advanceUntilIdle()

            vm.removeTag(withoutId, "friends")
            vm.pickTag(withoutId, "solo")
            vm.acceptSuggestion(withoutId)
            vm.openTagPicker(withoutId)
            advanceUntilIdle()
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertNull(vm.state.value.tagPicker)

            ledger.readOnly = listOf(Finding("ConfigInvalid", true, null, null, emptyMap()))
            val readOnly = viewModel()
            advanceUntilIdle()

            readOnly.removeTag(suggested, "friends")
            readOnly.pickTag(suggested, "solo")
            readOnly.acceptSuggestion(suggested)
            readOnly.openTagPicker(suggested)
            advanceUntilIdle()
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertTrue(ledger.picked.isEmpty())
            assertNull(readOnly.state.value.tagPicker)
        }

    @Test
    fun `removing a tag hides it at once and a second tap records nothing`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.queue = listOf(tagged)
            val vm = viewModel()
            advanceUntilIdle()

            vm.removeTag(tagged, "friends")
            assertEquals(emptyList<String>(), vm.state.value.items.single().tags)
            vm.removeTag(tagged, "friends")
            advanceUntilIdle()

            assertEquals(
                listOf(Intent.RemoveTag(tagged.id!!, "friends")),
                store.state.pending.map { it.intent },
            )
        }

    @Test
    fun `accepting a suggestion twice picks it once`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.queue = listOf(suggested)
            val vm = viewModel()
            advanceUntilIdle()

            vm.acceptSuggestion(suggested)
            assertNull(vm.state.value.items.single().suggestedTag)
            vm.acceptSuggestion(suggested)
            advanceUntilIdle()

            assertEquals(listOf(suggested.id!! to "trip-lisbon"), ledger.picked)
        }

    @Test
    fun `confirming or dismissing hides a transaction whose tags changed since it was shown`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            ledger.queue = listOf(grocery, older).map { it.copy(tags = listOf("friends")) }
            vm.sync()
            advanceUntilIdle()
            vm.confirm(grocery)
            vm.dismiss(older)

            assertEquals(emptyList<QueueEntry>(), vm.state.value.items)
        }

    @Test
    fun `a reload shows the open tag picker's transaction as it is now`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()
            vm.openTagPicker(grocery)
            advanceUntilIdle()

            val now = grocery.copy(tags = listOf("friends"))
            ledger.queue = listOf(now, older)
            vm.sync()
            advanceUntilIdle()

            assertEquals(now, vm.state.value.tagPicker?.entry)
        }

    @Test
    fun `the tag picker closes once its transaction leaves the queue or edits stop`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.openTagPicker(grocery)
            advanceUntilIdle()
            ledger.queue = listOf(older)
            vm.sync()
            advanceUntilIdle()
            assertNull(vm.state.value.tagPicker)

            vm.openTagPicker(older)
            advanceUntilIdle()
            ledger.readOnly = listOf(Finding("ConfigInvalid", true, null, null, emptyMap()))
            vm.sync()
            advanceUntilIdle()
            assertNull(vm.state.value.tagPicker)

            ledger.readOnly = emptyList()
            vm.sync()
            advanceUntilIdle()
            vm.openTagPicker(older)
            advanceUntilIdle()
            ledger.loadError = LedgerError.Io("disk full")
            vm.sync()
            advanceUntilIdle()
            assertNull(vm.state.value.tagPicker)
        }

    @Test
    fun `a tag picker that fails to open shows the error`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            ledger.loadError = LedgerError.Io("disk full")
            vm.openTagPicker(grocery)
            advanceUntilIdle()

            assertNull(vm.state.value.tagPicker)
            assertEquals(LedgerError.Io("disk full"), vm.state.value.loadError)
        }

    @Test
    fun `a pick that fails shows the error and records nothing`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            ledger.loadError = LedgerError.Other("broken")
            vm.pickTag(grocery, "friends")
            advanceUntilIdle()

            assertEquals(LedgerError.Other("broken"), vm.state.value.loadError)
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
        }
}
