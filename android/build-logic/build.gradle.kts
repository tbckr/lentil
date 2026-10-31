plugins { `kotlin-dsl` }

dependencies { compileOnly(libs.android.gradle.api) }

gradlePlugin {
    plugins {
        register("rustAndroid") {
            id = "lentil.rust-android"
            implementationClass = "RustAndroidPlugin"
        }
    }
}
