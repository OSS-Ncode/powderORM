plugins {
    kotlin("jvm") version "2.0.21"
    `java-library`
    id("com.vanniktech.maven.publish") version "0.37.0"
}

java {
    toolchain { languageVersion.set(JavaLanguageVersion.of(17)) }
}

dependencies {
    // The Kotlin DSL wraps the Java (JNI) binding's com.powder classes.
    api(project(":powder-java"))
}

// Kotlin sources live under bindings/kotlin/src (dev/powder/Powder.kt).
sourceSets {
    main { kotlin { setSrcDirs(listOf("../../bindings/kotlin/src")) } }
}

mavenPublishing {
    publishToMavenCentral(com.vanniktech.maven.publish.SonatypeHost.CENTRAL_PORTAL)
    if (providers.gradleProperty("signingInMemoryKey").isPresent) {
        signAllPublications()
    }
    coordinates(project.group.toString(), "powder-kotlin", project.version.toString())
    pom {
        name.set("powder-kotlin")
        description.set("Kotlin DSL for the Powder engine — an ORM-style query builder over the JNI binding.")
        url.set("https://github.com/OSS-Ncode/powderORM")
        licenses { license { name.set("MIT"); url.set("https://opensource.org/licenses/MIT") } }
        developers { developer { id.set("oss-ncode"); name.set("Powder team") } }
        scm {
            url.set("https://github.com/OSS-Ncode/powderORM")
            connection.set("scm:git:https://github.com/OSS-Ncode/powderORM.git")
            developerConnection.set("scm:git:ssh://git@github.com/OSS-Ncode/powderORM.git")
        }
    }
}
