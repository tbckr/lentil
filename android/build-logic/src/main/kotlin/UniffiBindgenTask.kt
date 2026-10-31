import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.InputDirectory
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.PathSensitive
import org.gradle.api.tasks.PathSensitivity
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import org.gradle.work.DisableCachingByDefault

/**
 * Generates the Kotlin bindings from a built library with the workspace's own `uniffi-bindgen`, so
 * the bindings always match the UniFFI version the library was built with.
 */
@DisableCachingByDefault(because = "Generating is faster than caching")
abstract class UniffiBindgenTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations

    @get:Internal abstract val workspace: DirectoryProperty

    @get:Input abstract val crate: Property<String>

    /** The `jniLibs` output of [CargoNdkTask]. */
    @get:InputDirectory
    @get:PathSensitive(PathSensitivity.RELATIVE)
    abstract val jniLibs: DirectoryProperty

    @get:Input abstract val abi: Property<String>

    @get:Input abstract val libraryName: Property<String>

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    @TaskAction
    fun generate() {
        val output = outputDir.get().asFile
        output.deleteRecursively()
        output.mkdirs()
        val library = jniLibs.get().file("${abi.get()}/lib${libraryName.get()}.so").asFile
        exec.exec {
            workingDir = workspace.get().asFile
            executable = "cargo"
            args(
                "run",
                "--locked",
                "--quiet",
                "--package",
                crate.get(),
                "--features",
                "bindgen",
                "--bin",
                "uniffi-bindgen",
                "--",
                "generate",
                "--library",
                library.absolutePath,
                "--language",
                "kotlin",
                "--out-dir",
                output.absolutePath,
                "--no-format",
                "--metadata-no-deps",
            )
        }
    }
}
