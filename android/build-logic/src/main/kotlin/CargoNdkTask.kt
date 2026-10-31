import java.io.ByteArrayOutputStream
import java.io.File
import javax.inject.Inject
import org.gradle.api.DefaultTask
import org.gradle.api.file.DirectoryProperty
import org.gradle.api.provider.ListProperty
import org.gradle.api.provider.Property
import org.gradle.api.tasks.Input
import org.gradle.api.tasks.Internal
import org.gradle.api.tasks.OutputDirectory
import org.gradle.api.tasks.TaskAction
import org.gradle.process.ExecOperations
import org.gradle.work.DisableCachingByDefault

/**
 * Runs `cargo ndk build` for each ABI into a `jniLibs` layout.
 *
 * Cargo tracks the workspace's sources itself and builds incrementally, so the task always runs and
 * leaves up-to-date checks to Cargo.
 */
@DisableCachingByDefault(because = "Cargo tracks its inputs and builds incrementally")
abstract class CargoNdkTask : DefaultTask() {
    @get:Inject abstract val exec: ExecOperations

    @get:Internal abstract val workspace: DirectoryProperty

    @get:Input abstract val crate: Property<String>

    @get:Input abstract val abis: ListProperty<String>

    @get:Input abstract val apiLevel: Property<Int>

    /** Release profile for release variants, the dev profile otherwise. */
    @get:Input abstract val release: Property<Boolean>

    @get:Internal abstract val ndkDirectory: DirectoryProperty

    @get:OutputDirectory abstract val outputDir: DirectoryProperty

    init {
        outputs.upToDateWhen { false }
    }

    @TaskAction
    fun build() {
        val root = workspace.get().asFile
        val output = outputDir.get().asFile
        // Drop libraries of ABIs that are no longer built.
        output.deleteRecursively()
        val arguments = buildList {
            add("ndk")
            add("--platform")
            add(apiLevel.get().toString())
            for (abi in abis.get()) {
                add("--target")
                add(abi)
            }
            add("--output-dir")
            add(output.absolutePath)
            add("build")
            add("--locked")
            add("--package")
            add(crate.get())
            add("--lib")
            if (release.get()) add("--release")
        }
        exec.exec {
            workingDir = root
            executable = "cargo"
            args(arguments)
            environment("ANDROID_NDK_HOME", ndkDirectory.get().asFile.absolutePath)
            // No build paths in the libraries, so the APK is reproducible (CONTRIBUTING:
            // Supply chain). Applies to the Android targets only, not to build scripts.
            environment("CARGO_ENCODED_RUSTFLAGS", remapFlags(root).joinToString("\u001f"))
        }
    }

    private fun remapFlags(root: File): List<String> {
        val cargoHome =
            System.getenv("CARGO_HOME")?.let(::File)
                ?: File(System.getProperty("user.home"), ".cargo")
        return listOf(
            "--remap-path-prefix=${root.absolutePath}=.",
            "--remap-path-prefix=${cargoHome.absolutePath}=/cargo",
            // With the rust-src component installed, rustc points the standard library's
            // paths at the local toolchain instead of its neutral /rustc/<commit> prefix.
            "--remap-path-prefix=${rustSysroot(root)}=/rustc",
        )
    }

    private fun rustSysroot(root: File): String {
        val output = ByteArrayOutputStream()
        exec.exec {
            workingDir = root
            executable = "rustc"
            args("--print", "sysroot")
            standardOutput = output
        }
        return output.toString(Charsets.UTF_8).trim()
    }
}
