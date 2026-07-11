// Gradle build for the JVM bindings (Java + Kotlin), publishable to Maven Central.
// Sources live in their existing locations (crates/powder-java, bindings/kotlin);
// each module points its sourceSet at them so nothing had to move.
rootProject.name = "powder-jvm"

pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

include(":powder-java")
include(":powder-kotlin")
