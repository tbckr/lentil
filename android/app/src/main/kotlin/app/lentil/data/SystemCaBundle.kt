package app.lentil.data

import android.util.Base64
import java.io.File
import java.security.KeyStore
import java.security.cert.X509Certificate

/**
 * Exports the CA certificates Android trusts by default to a PEM file for the core's OpenSSL, which
 * cannot read Android's CA directories. Like Android's default network security configuration, only
 * system CAs count, not user-installed ones.
 */
class SystemCaBundle(private val file: File) {
    fun export(): File {
        val store = KeyStore.getInstance("AndroidCAStore").apply { load(null) }
        val certificates =
            store
                .aliases()
                .asSequence()
                .filter { it.startsWith("system:") }
                .mapNotNull { store.getCertificate(it) as? X509Certificate }
        val pem = buildString {
            for (certificate in certificates) {
                append("-----BEGIN CERTIFICATE-----\n")
                Base64.encodeToString(certificate.encoded, Base64.NO_WRAP)
                    .chunked(PEM_LINE)
                    .forEach { append(it).append('\n') }
                append("-----END CERTIFICATE-----\n")
            }
        }
        val temporary = File(file.parentFile, "${file.name}.tmp")
        temporary.writeText(pem)
        check(temporary.renameTo(file)) { "cannot replace ${file.name}" }
        return file
    }

    private companion object {
        const val PEM_LINE = 64
    }
}
