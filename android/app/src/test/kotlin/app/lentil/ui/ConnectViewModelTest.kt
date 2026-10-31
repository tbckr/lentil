package app.lentil.ui

import app.lentil.data.Connection
import app.lentil.data.LedgerError
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test

@OptIn(ExperimentalCoroutinesApi::class)
class ConnectViewModelTest {
    @get:Rule val main = MainDispatcherRule()

    private val ledger = FakeLedger()
    private val settings = FakeSettings(connection = null)
    private val secrets = FakeSecrets(stored = null)

    private fun filled(url: String = "https://git.example.org/alex/ledger.git") =
        ConnectViewModel(ledger, settings, secrets).apply {
            update {
                it.copy(
                    url = url,
                    username = "alex",
                    token = "secret-token",
                    authorName = "Alex Example",
                    authorEmail = "alex@example.org",
                )
            }
        }

    @Test
    fun `only HTTPS remotes are accepted`() =
        runTest(main.dispatcher) {
            val vm = filled(url = "ssh://git@git.example.org/alex/ledger.git")

            vm.connect()
            advanceUntilIdle()

            assertEquals(ConnectProblem.NotHttps, vm.state.value.problem)
            assertNull(ledger.cloned)
        }

    @Test
    fun `an incomplete form does nothing`() =
        runTest(main.dispatcher) {
            val vm = filled().apply { update { it.copy(token = " ") } }

            vm.connect()
            advanceUntilIdle()

            assertFalse(vm.state.value.complete)
            assertNull(ledger.cloned)
        }

    @Test
    fun `connecting clones and keeps the settings and the token`() =
        runTest(main.dispatcher) {
            val vm = filled()

            vm.connect()
            advanceUntilIdle()

            assertTrue(vm.state.value.connected)
            assertEquals("", vm.state.value.token)
            assertEquals("https://git.example.org/alex/ledger.git", ledger.cloned?.first)
            assertEquals("alex", ledger.cloned?.second?.username)
            assertEquals(
                Connection(
                    url = "https://git.example.org/alex/ledger.git",
                    username = "alex",
                    authorName = "Alex Example",
                    authorEmail = "alex@example.org",
                ),
                settings.connection,
            )
            assertEquals("secret-token", secrets.token())
        }

    @Test
    fun `a failed clone keeps the form and stores nothing`() =
        runTest(main.dispatcher) {
            ledger.cloneError = LedgerError.Auth
            val vm = filled()

            vm.connect()
            advanceUntilIdle()

            assertEquals(ConnectProblem.Failed(LedgerError.Auth), vm.state.value.problem)
            assertFalse(vm.state.value.connected)
            assertEquals("secret-token", vm.state.value.token)
            assertNull(settings.connection)
            assertNull(secrets.token())
        }

    @Test
    fun `credentials never show in logs`() {
        val credentials = app.lentil.data.Credentials("alex", "secret-token")

        assertFalse(credentials.toString().contains("secret-token"))
    }
}
