package app.lentil.ui

import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.ViewModelStore
import androidx.lifecycle.viewmodel.initializer
import androidx.lifecycle.viewmodel.viewModelFactory
import app.lentil.data.Finding
import app.lentil.data.Intent
import app.lentil.data.IntentOutcome
import app.lentil.data.LedgerError
import app.lentil.data.RuleWords
import app.lentil.data.StoredIntent
import app.lentil.data.usableOn
import kotlin.time.Duration.Companion.milliseconds
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Account changes: the core's correction, the rule gesture and its preview (PRD: FR-D3, FR-D14).
 */
@OptIn(ExperimentalCoroutinesApi::class)
class QueueViewModelAccountTest : QueueViewModelTestBase() {
    private val streamflix = RuleWords(emptyList(), listOf("streamflix"))
    private val moveOlder =
        Intent.SuggestAccount(older.id!!, 1, "Expenses:Uncategorized", "Expenses:Food")

    @Test
    fun `changing the account hides the transaction at once and remembers the account`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Groceries")
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(listOf("Expenses:Groceries"), vm.state.value.recentAccounts)
            advanceUntilIdle()

            assertEquals(
                listOf(FakeLedger.CorrectionCall(grocery.id!!, "Expenses:Groceries", null, null)),
                ledger.corrections,
            )
            assertEquals(
                listOf(
                    Intent.SetAccount(
                        id = grocery.id,
                        posting = 1,
                        old = "Expenses:Uncategorized",
                        new = "Expenses:Groceries",
                    ),
                    Intent.Confirm(grocery.id),
                ),
                store.state.pending.map { it.intent },
            )
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(2, vm.state.value.pendingCount)
        }

    @Test
    fun `choosing the same account records nothing`() =
        runTest(main.dispatcher) {
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Uncategorized")
            assertEquals(listOf(grocery, older), vm.state.value.items)
            advanceUntilIdle()

            assertTrue(ledger.corrections.isEmpty())
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
                    FakeLedger.CorrectionCall(grocery.id!!, "Expenses:Garden", "2026-10-18", null)
                ),
                ledger.corrections,
            )
            assertEquals(
                listOf(
                    Intent.OpenAccount("Expenses:Garden", "2026-10-18"),
                    Intent.SetAccount(grocery.id, 1, "Expenses:Uncategorized", "Expenses:Garden"),
                    Intent.Confirm(grocery.id),
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

            assertEquals("2026-10-30", ledger.corrections.single().openDate)
            assertEquals(
                Intent.OpenAccount("Expenses:Garden", "2026-10-30"),
                store.state.pending.first().intent,
            )
        }

    @Test
    fun `a plain account change records the core's correction`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.followUps = listOf(moveOlder)
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Food")
            advanceUntilIdle()

            assertEquals(
                listOf(
                    StoredIntent(
                        "key-1",
                        Intent.SetAccount(
                            grocery.id!!,
                            1,
                            "Expenses:Uncategorized",
                            "Expenses:Food",
                        ),
                        grocery.label,
                    ),
                    StoredIntent("key-2", Intent.Confirm(grocery.id), grocery.label),
                    StoredIntent("key-3", moveOlder, older.label),
                ),
                store.state.pending,
            )
            assertNull(ledger.corrections.single().rule)
            val notice = checkNotNull(vm.state.value.moved)
            assertEquals(1, notice.count)

            vm.clearMoved(notice)
            assertNull(vm.state.value.moved)
        }

    @Test
    fun `two changes that move as many give a notice each`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.followUps = listOf(moveOlder)
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Food")
            advanceUntilIdle()
            val first = checkNotNull(vm.state.value.moved)
            vm.changeAccount(older, "Expenses:Food")
            advanceUntilIdle()
            val second = checkNotNull(vm.state.value.moved)

            assertEquals(listOf(1, 1), listOf(first.count, second.count))
            assertNotEquals(first, second)
            // The end of the first notice leaves the second in place.
            vm.clearMoved(first)
            assertEquals(second, vm.state.value.moved)
            vm.clearMoved(second)
            assertNull(vm.state.value.moved)
        }

    @Test
    fun `a follow-up carries the label of its own transaction`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val shown = older.copy(payee = "Book Shop")
            ledger.queue = listOf(grocery, shown)
            val unseen =
                Intent.SuggestAccount("giro-ffffffffffff", 1, "Expenses:Uncategorized", "X:Y")
            ledger.followUps = listOf(moveOlder, unseen)
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Food", streamflix)
            advanceUntilIdle()

            // A transaction the queue does not show gets the corrected one's label.
            assertEquals(
                listOf(
                    grocery.label,
                    grocery.label,
                    grocery.label,
                    shown.label,
                    grocery.label,
                ),
                store.state.pending.map { it.label },
            )
            assertEquals(listOf(moveOlder, unseen), store.state.pending.drop(3).map { it.intent })
        }

    @Test
    fun `a gesture passes the selected words`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Subscriptions", streamflix)
            advanceUntilIdle()

            assertEquals(streamflix, ledger.corrections.single().rule)
            assertTrue(
                Intent.AddRule(purpose = "streamflix", account = "Expenses:Subscriptions") in
                    store.state.pending.map { it.intent }
            )
        }

    @Test
    fun `a gesture on the current account still adds its rule`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val suggested = grocery.copy(counterAccount = "Expenses:Groceries")
            ledger.queue = listOf(suggested, older)
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(suggested, "Expenses:Groceries", streamflix)
            assertEquals(listOf(older), vm.state.value.items)
            advanceUntilIdle()

            assertEquals(streamflix, ledger.corrections.single().rule)
            assertTrue(Intent.Confirm(grocery.id!!) in store.state.pending.map { it.intent })
        }

    @Test
    fun `nothing moved shows no notice`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Groceries", streamflix)
            advanceUntilIdle()

            assertEquals(3, store.state.pending.size)
            assertNull(vm.state.value.moved)
        }

    @Test
    fun `opening an account with a gesture opens it first`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.followUps = listOf(moveOlder)
            val vm = viewModel()
            advanceUntilIdle()

            vm.openAccount(grocery, "Expenses:Subscriptions", streamflix)
            advanceUntilIdle()

            assertEquals(
                FakeLedger.CorrectionCall(
                    grocery.id!!,
                    "Expenses:Subscriptions",
                    "2026-10-18",
                    streamflix,
                ),
                ledger.corrections.single(),
            )
            assertEquals(
                listOf(
                    Intent.OpenAccount("Expenses:Subscriptions", "2026-10-18"),
                    Intent.SetAccount(
                        grocery.id,
                        1,
                        "Expenses:Uncategorized",
                        "Expenses:Subscriptions",
                    ),
                    Intent.Confirm(grocery.id),
                    Intent.AddRule(purpose = "streamflix", account = "Expenses:Subscriptions"),
                    moveOlder,
                ),
                store.state.pending.map { it.intent },
            )
            assertEquals(1, vm.state.value.moved?.count)
        }

    @Test
    fun `a preview waits until the choice rests`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.preview = 3
            val vm = viewModel()
            advanceUntilIdle()

            vm.previewRule(grocery, "Expenses:Groceries", null, streamflix)
            advanceTimeBy(299.milliseconds)
            vm.previewRule(grocery, "Expenses:Garden", vm.openingDate(), streamflix)
            advanceTimeBy(299.milliseconds)
            assertTrue(ledger.previews.isEmpty())
            assertNull(vm.state.value.rulePreview)

            advanceUntilIdle()
            assertEquals(
                listOf(
                    FakeLedger.CorrectionCall(
                        grocery.id!!,
                        "Expenses:Garden",
                        "2026-10-18",
                        streamflix,
                    )
                ),
                ledger.previews,
            )
            assertEquals(3, vm.state.value.rulePreview)
        }

    @Test
    fun `only the latest preview lands`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            val first = CompletableDeferred<Unit>()
            ledger.previewGate = first
            ledger.preview = 1
            vm.previewRule(grocery, "Expenses:Groceries", null, streamflix)
            advanceUntilIdle()
            ledger.previewGate = null
            ledger.preview = 2
            vm.previewRule(grocery, "Expenses:Food", null, streamflix)
            advanceUntilIdle()
            assertEquals(2, vm.state.value.rulePreview)

            first.complete(Unit)
            advanceUntilIdle()
            assertEquals(2, vm.state.value.rulePreview)
        }

    @Test
    fun `quick corrections are computed one after the other`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            val gate = CompletableDeferred<Unit>()
            ledger.correctionGate = gate
            vm.changeAccount(grocery, "Expenses:Groceries")
            vm.changeAccount(older, "Expenses:Groceries")
            advanceUntilIdle()
            gate.complete(Unit)
            advanceUntilIdle()

            assertEquals(
                listOf(
                    emptyList(),
                    listOf(
                        Intent.SetAccount(
                            grocery.id!!,
                            1,
                            "Expenses:Uncategorized",
                            "Expenses:Groceries",
                        ),
                        Intent.Confirm(grocery.id),
                    ),
                ),
                ledger.correctedOn.map { pending -> pending.map { it.intent } },
            )
            assertEquals(4, store.state.pending.size)
        }

    @Test
    fun `a reload during a correction keeps the transaction hidden`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            val gate = CompletableDeferred<Unit>()
            ledger.correctionGate = gate
            vm.changeAccount(grocery, "Expenses:Groceries")
            vm.sync()
            advanceUntilIdle()
            assertEquals(listOf(older), vm.state.value.items)

            gate.complete(Unit)
            advanceUntilIdle()
            assertEquals(listOf(older), vm.state.value.items)
            assertEquals(2, store.state.pending.size)
        }

    @Test
    fun `a correction the core fails still records the user's own change`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            ledger.correctionError = LedgerError.NotCorrectable
            vm.openAccount(grocery, "Expenses:Garden", streamflix)
            advanceUntilIdle()

            // Without the rule, which only the core computes.
            assertEquals(
                listOf(
                    Intent.OpenAccount("Expenses:Garden", "2026-10-18"),
                    Intent.SetAccount(grocery.id!!, 1, "Expenses:Uncategorized", "Expenses:Garden"),
                    Intent.Confirm(grocery.id),
                ),
                store.state.pending.map { it.intent },
            )
            assertEquals(List(3) { grocery.label }, store.state.pending.map { it.label })
            assertEquals(LedgerError.NotCorrectable, vm.state.value.loadError)
            assertEquals(listOf(older), vm.state.value.items)
            assertNull(vm.state.value.moved)

            // Nothing stays hidden: once the sync turns the change into conflicts, the
            // transaction is open again and shows.
            ledger.syncError = null
            ledger.outcomes = { pending ->
                pending.associate { it.key to IntentOutcome.Conflict("NotFound", null) }
            }
            vm.sync()
            advanceUntilIdle()
            assertEquals(listOf(grocery, older), vm.state.value.items)
        }

    @Test
    fun `corrections run to their end after the view model is cleared`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val owner = ViewModelStore()
            val factory = viewModelFactory { initializer { viewModel() } }
            val vm = ViewModelProvider.create(owner, factory)[QueueViewModel::class]
            advanceUntilIdle()

            // The first waits in the core, the second for the first.
            val gate = CompletableDeferred<Unit>()
            ledger.correctionGate = gate
            vm.changeAccount(grocery, "Expenses:Food")
            vm.changeAccount(older, "Expenses:Groceries")
            advanceUntilIdle()
            owner.clear()
            gate.complete(Unit)
            advanceUntilIdle()

            assertEquals(
                listOf(
                    Intent.SetAccount(grocery.id!!, 1, "Expenses:Uncategorized", "Expenses:Food"),
                    Intent.Confirm(grocery.id),
                    Intent.SetAccount(
                        older.id!!,
                        1,
                        "Expenses:Uncategorized",
                        "Expenses:Groceries",
                    ),
                    Intent.Confirm(older.id),
                ),
                store.state.pending.map { it.intent },
            )
        }

    @Test
    fun `a read-only ledger takes no account change or preview`() =
        runTest(main.dispatcher) {
            ledger.readOnly = listOf(Finding("OutsideProfile", true, "main.beancount", 3, mapOf()))
            val vm = viewModel()
            advanceUntilIdle()

            vm.changeAccount(grocery, "Expenses:Groceries", streamflix)
            vm.openAccount(older, "Expenses:Garden")
            vm.previewRule(grocery, "Expenses:Groceries", null, streamflix)
            advanceUntilIdle()

            assertEquals(listOf(grocery, older), vm.state.value.items)
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertTrue(ledger.corrections.isEmpty())
            assertTrue(ledger.previews.isEmpty())
        }
}
