import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.gradle.api.GradleException
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.artifacts.component.ModuleComponentIdentifier
import org.gradle.api.artifacts.dsl.DependencyHandler
import org.gradle.api.artifacts.result.ResolvedArtifactResult
import org.gradle.api.file.RegularFileProperty
import org.gradle.api.provider.ListProperty
import org.gradle.kotlin.dsl.create
import org.gradle.kotlin.dsl.register
import org.gradle.maven.MavenModule
import org.gradle.maven.MavenPomArtifact
import org.w3c.dom.Element

/** Which licenses the project's dependencies may have. */
abstract class LicenseCheckExtension {
    /** cargo-deny's configuration, whose `[licenses]` allowlist applies to Gradle as well. */
    abstract val denyToml: RegularFileProperty

    /** Modules exempt from the check, as `group:name`. */
    abstract val exempt: ListProperty<String>
}

/**
 * Registers `checkLicenses` for every module on a runtime classpath of the project: what the app
 * ships and what its tests run with. Build tools such as AGP, Lint and the Kotlin compiler are on
 * none of these classpaths and stay unchecked, like the Rust toolchain.
 */
class LicenseCheckPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val extension = project.extensions.create<LicenseCheckExtension>("licenseCheck")
        project.tasks.register<LicenseCheckTask>("checkLicenses") {
            group = "verification"
            description = "Checks the licenses of the runtime and test dependencies."
            denyToml.set(extension.denyToml)
            exempt.set(extension.exempt)
            // Only configuration may resolve dependencies, so the POMs are read then; the
            // configuration cache stores the result.
            declared.set(project.provider { declaredLicenses(project) })
        }
    }
}

/** The licenses that each module on a runtime classpath declares, by `group:name:version`. */
private fun declaredLicenses(project: Project): Map<String, List<String>> {
    val modules =
        project.configurations
            .filter { it.isCanBeResolved && it.name.endsWith("RuntimeClasspath") }
            .flatMap { it.incoming.resolutionResult.allComponents }
            .mapNotNull { it.id as? ModuleComponentIdentifier }
            .map { Coordinates(it.group, it.module, it.version) }
            .toSet()
    val poms = Poms(project.dependencies)
    return modules.associate { it.toString() to poms.licenses(it) }.toSortedMap()
}

private data class Coordinates(val group: String, val name: String, val version: String) {
    override fun toString() = "$group:$name:$version"
}

/** A POM's licenses, each as `name|url`, and its parent. */
private class Pom(val licenses: List<String>, val parent: Coordinates?)

/**
 * Reads POMs through Gradle, which resolves them from the build's repositories and verifies them.
 */
private class Poms(private val dependencies: DependencyHandler) {
    private val cache = mutableMapOf<Coordinates, Pom>()

    /**
     * The licenses of the module's POM or, as Maven inherits them, of the nearest parent with any.
     */
    fun licenses(module: Coordinates): List<String> {
        var pom = read(module)
        while (pom.licenses.isEmpty()) pom = read(pom.parent ?: break)
        return pom.licenses
    }

    private fun read(module: Coordinates) = cache.getOrPut(module) { parse(file(module)) }

    private fun file(module: Coordinates): File {
        val result =
            dependencies
                .createArtifactResolutionQuery()
                .forModule(module.group, module.name, module.version)
                .withArtifacts(MavenModule::class.java, MavenPomArtifact::class.java)
                .execute()
        val artifact =
            result.resolvedComponents
                .singleOrNull()
                ?.getArtifacts(MavenPomArtifact::class.java)
                ?.singleOrNull()
        return (artifact as? ResolvedArtifactResult)?.file
            ?: throw GradleException("checkLicenses: no POM for $module")
    }
}

private fun parse(file: File): Pom {
    val factory = DocumentBuilderFactory.newInstance()
    // A POM needs no DTD, so none can pull in external entities.
    factory.setFeature("http://apache.org/xml/features/disallow-doctype-decl", true)
    val project = factory.newDocumentBuilder().parse(file).documentElement
    val licenses =
        project.child("licenses")?.children("license").orEmpty().map {
            "${it.text("name")}|${it.text("url")}"
        }
    val parent =
        project.child("parent")?.let {
            Coordinates(it.text("groupId"), it.text("artifactId"), it.text("version"))
        }
    return Pom(licenses, parent)
}

private fun Element.children(tag: String): List<Element> =
    (0 until childNodes.length)
        .map { childNodes.item(it) }
        .filterIsInstance<Element>()
        .filter { it.tagName == tag }

private fun Element.child(tag: String) = children(tag).firstOrNull()

private fun Element.text(tag: String) = child(tag)?.textContent?.trim().orEmpty()
