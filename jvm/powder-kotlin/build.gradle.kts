plugins {
    kotlin("jvm") version "2.0.21"
    `java-library`
    `maven-publish`
    signing
}

java {
    toolchain { languageVersion.set(JavaLanguageVersion.of(17)) }
    withSourcesJar()
    withJavadocJar()
}

dependencies {
    // The Kotlin DSL wraps the Java (JNI) binding's com.powder classes.
    api(project(":powder-java"))
}

// Kotlin sources live under bindings/kotlin/src (dev/powder/Powder.kt).
sourceSets {
    main { kotlin { setSrcDirs(listOf("../../bindings/kotlin/src")) } }
}

publishing {
    publications {
        create<MavenPublication>("maven") {
            from(components["java"])
            artifactId = "powder-kotlin"
            pom {
                name.set("powder-kotlin")
                description.set("Kotlin DSL for the Powder engine — an ORM-style query builder over the JNI binding.")
                url.set("https://github.com/OSS-Ncode/powderORM")
                licenses { license { name.set("MIT"); url.set("https://opensource.org/licenses/MIT") } }
                developers { developer { id.set("oss-ncode"); name.set("Powder team") } }
                scm {
                    url.set("https://github.com/OSS-Ncode/powderORM")
                    connection.set("scm:git:https://github.com/OSS-Ncode/powderORM.git")
                }
            }
        }
    }
    repositories {
        maven {
            name = "ossrh"
            url = uri(providers.gradleProperty("ossrhUrl")
                .getOrElse("https://s01.oss.sonatype.org/service/local/staging/deploy/maven2/"))
            credentials {
                username = providers.gradleProperty("ossrhUsername").orNull
                password = providers.gradleProperty("ossrhPassword").orNull
            }
        }
    }
}

signing {
    isRequired = providers.gradleProperty("signingKey").isPresent
    useInMemoryPgpKeys(
        providers.gradleProperty("signingKey").orNull,
        providers.gradleProperty("signingPassword").orNull,
    )
    sign(publishing.publications["maven"])
}
