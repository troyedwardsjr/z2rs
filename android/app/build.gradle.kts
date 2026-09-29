plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

// Release builds pass the tag's version as -Pz2rsVersion=0.4.0. The version
// code is derived from it (major * 10000 + minor * 100 + patch), so every
// release installs over the previous one.
val z2rsVersion = (findProperty("z2rsVersion") as String?)?.removePrefix("v") ?: "0.0.0"
val z2rsVersionCode = z2rsVersion.split(".").map { it.toIntOrNull() ?: 0 }
    .let { (it.getOrElse(0) { 0 } * 10000) + (it.getOrElse(1) { 0 } * 100) + it.getOrElse(2) { 0 } }
    .coerceAtLeast(1)

android {
    namespace = "com.z2rs.game"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.z2rs.game"
        minSdk = 26
        targetSdk = 35
        versionCode = z2rsVersionCode
        versionName = z2rsVersion

        ndk {
            // The only ABIs tools/android/build.sh produces.
            abiFilters += listOf("arm64-v8a", "x86_64")
        }
    }

    buildTypes {
        release {
            // Kept off: the JNI surface is small and R8 buys little here.
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        buildConfig = true
    }

    packaging {
        jniLibs {
            // Keep libraries uncompressed and page-aligned inside the APK
            // (loaded in place; nothing is extracted to disk).
            useLegacyPackaging = false
        }
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

kotlin {
    jvmToolchain(17)
}

dependencies {
    implementation(libs.games.activity)
    implementation(libs.androidx.appcompat)
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.customview)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.material)

    testImplementation(libs.junit)
}
