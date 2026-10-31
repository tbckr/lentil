import org.gradle.api.DefaultTask
import org.gradle.api.GradleException
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.MapProperty
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputFile
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.work.DisableCachingByDefault

/**
 * Checks that every module has a license from cargo-deny's allowlist, so the Gradle dependencies
 * follow the same rule as the Rust ones (CONTRIBUTING: Dependencies).
 *
 * A module that declares several licenses passes if one of them is allowed: Maven has no syntax for
 * license expressions, and POMs list alternatives this way, as JNA's does.
 */
@DisableCachingByDefault(because = "Checking is faster than caching")
abstract class LicenseCheckTask : DefaultTask() {
    /** cargo-deny's configuration; the `allow` array of its `[licenses]` table is the allowlist. */
    @get:InputFile
    @get:PathSensitive(PathSensitivity.NONE)
    abstract val denyToml: RegularFileProperty

    /** The licenses that each module declares, by `group:name:version`, each as `name|url`. */
    @get:Input abstract val declared: MapProperty<String, List<String>>

    /** Modules exempt from the check, as `group:name`. */
    @get:Input abstract val exempt: ListProperty<String>

    @TaskAction
    fun check() {
        val allowed = allowedLicenses(denyToml.get().asFile.readText())
        val exempt = exempt.get().toSet()
        val declared = declared.get()
        val problems = buildList {
            for ((module, licenses) in declared) {
                val ids = licenses.map { spdxId(it, allowed) }
                when {
                    module.substringBeforeLast(':') in exempt -> continue
                    licenses.isEmpty() -> add("$module declares no license")
                    ids.none { it in allowed } ->
                        add("$module: ${licenses.zip(ids).joinToString { describe(it) }}")
                }
            }
            val modules = declared.keys.map { it.substringBeforeLast(':') }.toSet()
            for (module in exempt - modules) add("the exemption of $module matches no module")
        }
        if (problems.isNotEmpty()) {
            throw GradleException(
                problems.joinToString(
                    "\n",
                    prefix = "Licenses outside the allowlist in deny.toml:\n",
                    postfix =
                        "\nAn unknown license that is in fact allowed goes into the tables in" +
                            " LicenseCheckTask.kt.",
                )
            )
        }
        logger.info("checkLicenses: ${declared.size} modules, ${exempt.size} exempt")
    }
}

/** The `allow` array of the `[licenses]` table in a cargo-deny configuration. */
private fun allowedLicenses(toml: String): Set<String> {
    val table =
        toml
            .lines()
            .map { it.substringBefore('#').trim() }
            .dropWhile { it != "[licenses]" }
            .drop(1)
            .takeWhile { !it.startsWith("[") }
            .joinToString("\n")
    val allow =
        Regex("""\ballow\s*=\s*\[([^]]*)]""").find(table)
            ?: throw GradleException("deny.toml has no allow array in [licenses]")
    return Regex(""""([^"]+)"""").findAll(allow.groupValues[1]).map { it.groupValues[1] }.toSet()
}

/**
 * The SPDX ID of a license given as `name|url`: the name itself if it is an allowed ID, otherwise
 * from the tables below, or `null` if they do not know it.
 */
private fun spdxId(license: String, allowed: Set<String>): String? {
    val name = license.substringBefore('|')
    return name.takeIf { it in allowed }
        ?: SPDX_BY_NAME[name.lowercase()]
        ?: SPDX_BY_URL[urlKey(license.substringAfter('|', ""))]
}

/** A license URL in lower case, without scheme, `www.`, trailing slash and file extension. */
private fun urlKey(url: String) =
    url.lowercase()
        .removePrefix("http://")
        .removePrefix("https://")
        .removePrefix("www.")
        .removeSuffix("/")
        .removeSuffix(".txt")
        .removeSuffix(".html")

private fun describe(license: Pair<String, String?>): String {
    val (declared, id) = license
    return id
        ?: "unknown license '${declared.substringBefore('|')}' (${declared.substringAfter('|')})"
}

/** License names that POMs use for SPDX IDs, lower case, as far as the dependencies need them. */
private val SPDX_BY_NAME =
    mapOf(
        "apache license, version 2.0" to "Apache-2.0",
        "the apache license, version 2.0" to "Apache-2.0",
        "the apache software license, version 2.0" to "Apache-2.0",
        "eclipse public license 1.0" to "EPL-1.0",
        "lgpl-2.1-or-later" to "LGPL-2.1-or-later",
        "mit license" to "MIT",
        "the mit license" to "MIT",
        "new bsd license" to "BSD-3-Clause",
    )

/** License URLs that identify a license whatever name a POM gives it. */
private val SPDX_BY_URL = mapOf("apache.org/licenses/license-2.0" to "Apache-2.0")
