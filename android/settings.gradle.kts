// z2rs Android app. The native game library (libz2rs_android.so) is built by
// tools/android/build.sh with cargo-ndk into app/src/main/jniLibs/<abi>/;
// Gradle only packages what it finds there.
pluginManagement {
    repositories {
        google {
            content {
                includeGroupByRegex("com\\.android.*")
                includeGroupByRegex("com\\.google.*")
                includeGroupByRegex("androidx.*")
            }
        }
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "z2rs"
include(":app")
