package app.lentil.data

import android.content.SharedPreferences
import androidx.core.content.edit

/** [SettingsStore] in [SharedPreferences]. */
class PreferencesSettingsStore(private val preferences: SharedPreferences) : SettingsStore {
    override var deviceId: String?
        get() = preferences.getString(DEVICE_ID, null)
        set(value) = preferences.edit { putString(DEVICE_ID, value) }

    override var connection: Connection?
        get() {
            val url = preferences.getString(URL, null) ?: return null
            return Connection(
                url = url,
                username = preferences.getString(USERNAME, "").orEmpty(),
                authorName = preferences.getString(AUTHOR_NAME, "").orEmpty(),
                authorEmail = preferences.getString(AUTHOR_EMAIL, "").orEmpty(),
            )
        }
        set(value) = preferences.edit {
            putString(URL, value?.url)
            putString(USERNAME, value?.username)
            putString(AUTHOR_NAME, value?.authorName)
            putString(AUTHOR_EMAIL, value?.authorEmail)
        }

    override fun recentAccounts(): List<String> =
        preferences.getString(RECENT, null)?.split('\n')?.filter { it.isNotEmpty() }.orEmpty()

    override fun rememberAccount(account: String) {
        val recent =
            (listOf(account) + recentAccounts().filterNot { it == account }).take(MAX_RECENT)
        preferences.edit { putString(RECENT, recent.joinToString("\n")) }
    }

    private companion object {
        const val DEVICE_ID = "device_id"
        const val URL = "remote_url"
        const val USERNAME = "remote_username"
        const val AUTHOR_NAME = "author_name"
        const val AUTHOR_EMAIL = "author_email"
        const val RECENT = "recent_accounts"
        const val MAX_RECENT = 8
    }
}
