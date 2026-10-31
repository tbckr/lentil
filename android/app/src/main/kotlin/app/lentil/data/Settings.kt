package app.lentil.data

/** The repository the app is connected to, without the token. */
data class Connection(
    val url: String,
    val username: String,
    val authorName: String,
    val authorEmail: String,
)

/** Plain settings on the device. */
interface SettingsStore {
    var deviceId: String?
    var connection: Connection?

    /** Accounts the user picked recently, most recent first. */
    fun recentAccounts(): List<String>

    fun rememberAccount(account: String)
}

/** The Git token, kept in the system keystore (PRD: Security). */
interface SecretStore {
    fun hasToken(): Boolean

    fun token(): String?

    fun saveToken(token: String)

    fun clear()
}
