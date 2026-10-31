package app.lentil.data

import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.core.content.edit
import java.security.GeneralSecurityException
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Encrypts the token with an AES-GCM key that never leaves the Android Keystore; only the
 * ciphertext and its IV are stored in [preferences].
 */
class KeystoreSecretStore(private val preferences: SharedPreferences) : SecretStore {
    override fun hasToken(): Boolean = preferences.contains(TOKEN)

    /** The token, or `null` if none is stored or it cannot be decrypted any more. */
    @Suppress("SwallowedException")
    override fun token(): String? {
        val stored = preferences.getString(TOKEN, null) ?: return null
        return try {
            val bytes = Base64.decode(stored, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(
                Cipher.DECRYPT_MODE,
                key(),
                GCMParameterSpec(TAG_BITS, bytes, 0, IV_BYTES),
            )
            String(cipher.doFinal(bytes, IV_BYTES, bytes.size - IV_BYTES), Charsets.UTF_8)
        } catch (e: GeneralSecurityException) {
            // The key is gone, for example after the device's lock screen was reset; the user
            // signs in again.
            null
        } catch (e: IllegalArgumentException) {
            null
        }
    }

    override fun saveToken(token: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val iv = cipher.iv
        check(iv.size == IV_BYTES) { "unexpected IV length" }
        val encrypted = iv + cipher.doFinal(token.toByteArray(Charsets.UTF_8))
        preferences.edit { putString(TOKEN, Base64.encodeToString(encrypted, Base64.NO_WRAP)) }
    }

    override fun clear() {
        preferences.edit { remove(TOKEN) }
        KeyStore.getInstance(KEYSTORE).apply { load(null) }.deleteEntry(ALIAS)
    }

    private fun key(): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getKey(ALIAS, null) as? SecretKey)?.let {
            return it
        }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                    ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(KEY_BITS)
                .build()
        )
        return generator.generateKey()
    }

    private companion object {
        const val KEYSTORE = "AndroidKeyStore"
        const val ALIAS = "lentil-remote-token"
        const val TRANSFORMATION = "AES/GCM/NoPadding"
        const val TOKEN = "remote_token"
        const val KEY_BITS = 256
        const val TAG_BITS = 128
        const val IV_BYTES = 12
    }
}
