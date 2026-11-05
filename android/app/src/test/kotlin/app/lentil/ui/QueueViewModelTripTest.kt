package app.lentil.ui

import app.lentil.data.Finding
import app.lentil.data.Intent
import app.lentil.data.LedgerError
import app.lentil.data.StoredIntent
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import kotlinx.coroutines.test.UnconfinedTestDispatcher
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Starting and ending trips (PRD: FR-D13). */
@OptIn(ExperimentalCoroutinesApi::class)
class QueueViewModelTripTest : QueueViewModelTestBase() {
    private val tripId = "k7p2q9xd-01jc8m2w9z4f7q3r5t6v8x0y1a"

    @Test
    fun `starting a trip records start-trip and shows it running`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            ledger.runningTrip = "lisbon"
            vm.startTrip("lisbon")
            assertEquals("lisbon", vm.state.value.runningTrip)
            advanceUntilIdle()

            assertEquals(
                listOf(
                    StoredIntent(
                        "key-1",
                        Intent.StartTrip(tripId, "2026-10-30", "lisbon"),
                        "2026-10-30",
                    )
                ),
                store.state.pending,
            )
            assertEquals("lisbon", vm.state.value.runningTrip)
        }

    @Test
    fun `ending a trip records end-trip`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.runningTrip = "lisbon"
            val vm = viewModel()
            advanceUntilIdle()
            assertEquals("lisbon", vm.state.value.runningTrip)

            ledger.runningTrip = null
            vm.endTrip()
            assertNull(vm.state.value.runningTrip)
            advanceUntilIdle()

            assertEquals(
                listOf(StoredIntent("key-1", Intent.EndTrip(tripId, "2026-10-30"), "2026-10-30")),
                store.state.pending,
            )
            assertNull(vm.state.value.runningTrip)
        }

    @Test
    fun `an invalid trip name records nothing`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            for (name in listOf("", "Lisbon", "a b", "ä")) {
                assertEquals(name, false, vm.isPlainName(name))
                vm.startTrip(name)
            }
            advanceUntilIdle()

            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertNull(vm.state.value.runningTrip)
            assertTrue(vm.isPlainName("lisbon-2026"))
        }

    @Test
    fun `no trip starts without a device ID or on a read-only ledger`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val withoutDevice = viewModel(deviceId = null)
            advanceUntilIdle()
            withoutDevice.startTrip("lisbon")
            advanceUntilIdle()
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertNull(withoutDevice.state.value.runningTrip)

            ledger.readOnly = listOf(Finding("ConfigInvalid", true, null, null, emptyMap()))
            val readOnly = viewModel()
            advanceUntilIdle()
            readOnly.startTrip("lisbon")
            advanceUntilIdle()
            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertNull(readOnly.state.value.runningTrip)
        }

    @Test
    fun `no trip starts before the first load tells whether one runs`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            ledger.runningTrip = "lisbon"
            val vm = viewModel()

            vm.startTrip("porto")
            assertNull(vm.state.value.runningTrip)
            advanceUntilIdle()

            assertEquals(emptyList<StoredIntent>(), store.state.pending)
            assertEquals("lisbon", vm.state.value.runningTrip)
        }

    @Test
    fun `a reload that started before a trip does not undo it`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()
            // The reload after this sync waits at the gate, with the ledger as it was: no trip.
            val gate = CompletableDeferred<Unit>()
            ledger.loadGate = gate
            vm.sync()
            advanceUntilIdle()
            ledger.loadGate = null
            ledger.runningTrip = "lisbon"
            val shown = mutableListOf<String?>()
            backgroundScope.launch(UnconfinedTestDispatcher(testScheduler)) {
                vm.state.map { it.runningTrip }.distinctUntilChanged().collect { shown += it }
            }

            gate.complete(Unit)
            vm.startTrip("lisbon")
            advanceUntilIdle()

            assertEquals(listOf(null, "lisbon"), shown)
        }

    @Test
    fun `ending without a running trip records nothing`() =
        runTest(main.dispatcher) {
            ledger.syncError = LedgerError.Network
            val vm = viewModel()
            advanceUntilIdle()

            vm.endTrip()
            advanceUntilIdle()

            assertEquals(emptyList<StoredIntent>(), store.state.pending)
        }
}
