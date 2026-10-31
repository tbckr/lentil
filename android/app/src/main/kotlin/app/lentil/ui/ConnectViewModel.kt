package app.lentil.ui

import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import app.lentil.data.Connection
import app.lentil.data.Credentials
import app.lentil.data.LedgerError
import app.lentil.data.LedgerException
import app.lentil.data.LedgerRepository
import app.lentil.data.SecretStore
import app.lentil.data.SettingsStore
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

/** The connect form (PRD: N4, FR-F7). */
data class ConnectUiState(
    val url: String = "",
    val username: String = "",
    val token: String = "",
    val authorName: String = "",
    val authorEmail: String = "",
    val connecting: Boolean = false,
    val problem: ConnectProblem? = null,
    val connected: Boolean = false,
) {
    val complete: Boolean
        get() = listOf(url, username, token, authorName, authorEmail).all { it.isNotBlank() }
}

/** Why connecting did not work. */
sealed interface ConnectProblem {
    /** Only HTTPS remotes are supported on Android for now. */
    data object NotHttps : ConnectProblem

    data class Failed(val error: LedgerError) : ConnectProblem
}

/** Clones the repository and keeps the remote, the author and the token (in the keystore). */
class ConnectViewModel(
    private val ledger: LedgerRepository,
    private val settings: SettingsStore,
    private val secrets: SecretStore,
) : ViewModel() {
    private val mutableState =
        MutableStateFlow(
            settings.connection?.let {
                ConnectUiState(
                    url = it.url,
                    username = it.username,
                    authorName = it.authorName,
                    authorEmail = it.authorEmail,
                )
            } ?: ConnectUiState()
        )
    val state: StateFlow<ConnectUiState> = mutableState.asStateFlow()

    fun update(transform: (ConnectUiState) -> ConnectUiState) {
        mutableState.update { transform(it).copy(problem = null) }
    }

    fun connect() {
        val form = mutableState.value
        if (form.connecting || !form.complete) return
        val url = form.url.trim()
        if (!url.startsWith("https://")) {
            mutableState.update { it.copy(problem = ConnectProblem.NotHttps) }
            return
        }
        mutableState.update { it.copy(connecting = true, problem = null) }
        viewModelScope.launch {
            val credentials = Credentials(form.username.trim(), form.token.trim())
            val problem =
                try {
                    ledger.clone(url, credentials)
                    settings.connection =
                        Connection(
                            url = url,
                            username = credentials.username,
                            authorName = form.authorName.trim(),
                            authorEmail = form.authorEmail.trim(),
                        )
                    secrets.saveToken(credentials.token)
                    null
                } catch (e: LedgerException) {
                    ConnectProblem.Failed(e.error)
                }
            mutableState.update {
                it.copy(
                    connecting = false,
                    problem = problem,
                    connected = problem == null,
                    token = if (problem == null) "" else it.token,
                )
            }
        }
    }
}
