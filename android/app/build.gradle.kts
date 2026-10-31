plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
    alias(libs.plugins.kotlin.serialization)
    id("lentil.rust-android")
    id("lentil.license-check")
}

android {
    namespace = "app.lentil"
    compileSdk {
        version =
            release(libs.versions.android.compileSdk.get().toInt()) {
                minorApiLevel = libs.versions.android.compileSdkMinor.get().toInt()
            }
    }
    buildToolsVersion = libs.versions.android.buildTools.get()
    ndkVersion = libs.versions.android.ndk.get()

    defaultConfig {
        applicationId = "app.lentil"
        minSdk = libs.versions.android.minSdk.get().toInt()
        targetSdk = libs.versions.android.targetSdk.get().toInt()
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            // TODO: enable R8 with keep rules for JNA and the generated bindings.
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // Lets Android 13+ offer the app's languages in the system settings.
    androidResources { generateLocaleConfig = true }

    buildFeatures {
        buildConfig = true
        compose = true
    }

    lint {
        warningsAsErrors = true
        abortOnError = true
        lintConfig = file("lint.xml")
    }

    // Only Google Play reads this block; F-Droid builds must match the APK byte for byte.
    dependenciesInfo {
        includeInApk = false
        includeInBundle = false
    }
}

kotlin { compilerOptions { jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17 } }

rust {
    workspace = rootProject.layout.projectDirectory.dir("..")
    crate = "lentil-ffi"
    libraryName = "lentil_ffi"
    // arm64-v8a for devices, x86_64 for the emulator (PRD: Platforms).
    abis = listOf("arm64-v8a", "x86_64")
}

// The licenses of the runtime and test dependencies, against cargo-deny's allowlist
// (CONTRIBUTING: Dependencies).
licenseCheck {
    denyToml = rootProject.layout.projectDirectory.file("../deny.toml")
    // EPL-1.0, which GPL-3.0-only does not admit; JUnit only runs the unit tests and never ships.
    exempt.add("junit:junit")
}

// Lockfile for reproducible resolution and for osv-scanner (CONTRIBUTING: Vulnerability scanning).
dependencyLocking { lockAllConfigurations() }

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.kotlinx.serialization.json)
    // The generated UniFFI bindings call the Rust library through JNA.
    implementation(libs.jna) { artifact { type = "aar" } }

    testImplementation(libs.junit)
    testImplementation(libs.kotlinx.coroutines.test)
}
