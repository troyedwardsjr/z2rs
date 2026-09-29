# JNI entry points: the Rust library binds these by name.
-keep class com.z2rs.game.NativeBridge { *; }
# GameActivity's Java half is called from native code (android-activity).
-keep class com.google.androidgamesdk.** { *; }
-keep class com.z2rs.game.GameActivity { *; }
