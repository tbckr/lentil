import com.android.build.api.variant.ApplicationAndroidComponentsExtension
import org.gradle.api.Plugin
import org.gradle.api.Project
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.kotlin.dsl.create
import org.gradle.kotlin.dsl.getByType
import org.gradle.kotlin.dsl.register

/** What the app embeds from the Cargo workspace. */
abstract class RustExtension {
    /** Root of the Cargo workspace. */
    abstract val workspace: DirectoryProperty

    /** Cargo package of the library, such as `lentil-ffi`. */
    abstract val crate: Property<String>

    /** Library name without `lib` and `.so`, such as `lentil_ffi`. */
    abstract val libraryName: Property<String>

    /** Android ABIs to build, such as `arm64-v8a`; the first one feeds UniFFI. */
    abstract val abis: ListProperty<String>
}

/**
 * Builds a Rust library with cargo-ndk for every ABI of each variant, and generates its Kotlin
 * bindings with the workspace's `uniffi-bindgen`, through AGP's Variant API.
 */
class RustAndroidPlugin : Plugin<Project> {
    override fun apply(project: Project) {
        val rust = project.extensions.create<RustExtension>("rust")
        project.plugins.withId("com.android.application") {
            val components = project.extensions.getByType<ApplicationAndroidComponentsExtension>()
            // Package only the ABIs the library is built for, also for JNA's own native code.
            components.finalizeDsl { android ->
                android.defaultConfig.ndk.abiFilters.addAll(rust.abis.get())
            }
            components.onVariants { variant ->
                val suffix = variant.name.replaceFirstChar(Char::uppercaseChar)
                val cargo =
                    project.tasks.register<CargoNdkTask>("cargoNdk$suffix") {
                        workspace.set(rust.workspace)
                        crate.set(rust.crate)
                        abis.set(rust.abis)
                        apiLevel.set(variant.minSdk.apiLevel)
                        release.set(variant.buildType == "release")
                        ndkDirectory.set(components.sdkComponents.ndkDirectory)
                        outputDir.set(
                            project.layout.buildDirectory.dir("rust/jniLibs/${variant.name}")
                        )
                    }
                variant.sources.jniLibs?.addGeneratedSourceDirectory(cargo, CargoNdkTask::outputDir)

                val bindgen =
                    project.tasks.register<UniffiBindgenTask>("uniffiBindgen$suffix") {
                        workspace.set(rust.workspace)
                        crate.set(rust.crate)
                        jniLibs.set(cargo.flatMap { it.outputDir })
                        abi.set(rust.abis.map { it.first() })
                        libraryName.set(rust.libraryName)
                        outputDir.set(
                            project.layout.buildDirectory.dir("generated/uniffi/${variant.name}")
                        )
                    }
                (variant.sources.kotlin ?: variant.sources.java)?.addGeneratedSourceDirectory(
                    bindgen,
                    UniffiBindgenTask::outputDir,
                )
            }
        }
    }
}
