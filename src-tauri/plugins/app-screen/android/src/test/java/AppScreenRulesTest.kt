package com.takosumi.appscreen

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.io.File

/**
 * Checks the JVM-side rules against the fixtures `src/url-policy.ts` is tested
 * with, so Kotlin and TypeScript decide the same way.
 */
class AppScreenRulesTest {
    private fun fixture(name: String): String {
        val candidates = listOf(File("../$name"), File(name), File("../../$name"))
        return candidates.first { it.isFile }.readText()
    }

    private fun field(line: String, name: String): String? =
        Regex("\"$name\": \"([^\"]*)\"").find(line)?.groupValues?.get(1)

    @Test
    fun resumeCasesMatchTheSharedTable() {
        val lines = fixture("resume-url-cases.json").lines().filter { it.contains("\"appOrigin\"") }
        assertTrue(lines.size > 15)
        for (line in lines) {
            val url = field(line, "url")!!
            val origin = field(line, "appOrigin")!!
            val expected = line.contains("\"remember\": true")
            assertEquals(url, expected, AppScreenRules.isRememberable(url, origin))
        }
        assertFalse(
            AppScreenRules.isRememberable("https://app.example/" + "a".repeat(4_096), "https://app.example"),
        )
    }

    @Test
    fun credentialNamesMatchTheSharedTable() {
        val text = fixture("resume-url-cases.json")
        val block = text.substringAfter("\"credentialQueryNames\": [").substringBefore("]")
        val names = Regex("\"([a-z0-9]+)\"").findAll(block).map { it.groupValues[1] }.toSet()
        assertEquals(names, AppScreenRules.CREDENTIAL_QUERY_NAMES)
    }

    @Test
    fun originCasesMatchTheSharedTable() {
        val lines = fixture("navigation-policy-cases.json").lines()
            .filter { it.contains("\"allowedOrigins\"") && it.contains("\"url\": \"https://") }
        assertTrue(lines.size > 8)
        for (line in lines) {
            val url = field(line, "url")!!
            val allowed = Regex("\"allowedOrigins\": \\[([^\\]]*)]").find(line)!!.groupValues[1]
                .split(',').map { it.trim().trim('"') }.toSet()
            val decision = Regex("\"decision\": \"([a-z-]+)\"").find(line)!!.groupValues[1]
            if (line.contains("\"mainFrame\": false") || decision == "block") continue
            assertEquals(url, decision == "in-app", AppScreenRules.isAllowedOrigin(url, allowed))
        }
    }

    @Test
    fun originsAreWrittenAsUrlOriginDoes() {
        assertEquals("https://app.example", AppScreenRules.originOf("https://APP.example:443/x?y"))
        assertEquals("https://app.example:8443", AppScreenRules.originOf("https://app.example:8443/"))
        assertEquals("https://[::1]:8443", AppScreenRules.originOf("https://[::1]:8443/x"))
        assertNull(AppScreenRules.originOf("https://u@app.example/"))
        assertNull(AppScreenRules.originOf("about:blank"))
        assertNull(AppScreenRules.originOf("https://app.example:99999/"))
        assertTrue(AppScreenRules.isShellHost("TAURI.localhost."))
        assertTrue(AppScreenRules.hasRawUserInfo("https://:@app.example/"))
        assertTrue(AppScreenRules.hasRawUserInfo("http://u@app.example/"))
        assertFalse(AppScreenRules.hasRawUserInfo("https://app.example/@user"))
        assertTrue(AppScreenRules.isHttpsOrigin("https://accounts.example"))
        assertFalse(AppScreenRules.isHttpsOrigin("https://accounts.example/"))
        assertFalse(AppScreenRules.isHttpsOrigin("http://accounts.example"))
        assertFalse(AppScreenRules.isHttpsOrigin("https://ipc.localhost"))
    }

    @Test
    fun nativeAppLinksAreRechecked() {
        assertTrue(AppScreenRules.isAndroidPackage("com.yurucommu"))
        for (bad in listOf("com.takosumi", "COM.TAKOSUMI", "nodots", "1com.x", "com..x", "com.x-y", "")) {
            assertFalse(bad, AppScreenRules.isAndroidPackage(bad))
        }
        assertTrue(AppScreenRules.isCustomSchemeLink("yurucommu://connect?host_url=x"))
        for (bad in listOf(
            "takosumi://oauth/callback", "intent://x#Intent;scheme=a;end", "javascript:alert(1)",
            "https://x.example/", "tel:+81", "itms-services://x", "googlechrome://x",
            "Yurucommu://x", "yurucommu://x y", "y://x", "noscheme",
        )) {
            assertFalse(bad, AppScreenRules.isCustomSchemeLink(bad))
        }
        assertTrue(AppScreenRules.isHttpsLink("https://open.yurucommu.example/connect#host_url=x"))
        assertFalse(AppScreenRules.isHttpsLink("http://open.example/"))
        assertFalse(AppScreenRules.isHttpsLink("https://u@open.example/"))
        assertFalse(AppScreenRules.isHttpsLink("https://tauri.localhost/"))
        assertTrue(AppScreenRules.isClientName("Yurucommu"))
        assertFalse(AppScreenRules.isClientName("Chat\u202Eexe"))
        assertFalse(AppScreenRules.isClientName(" "))
        assertTrue(AppScreenRules.isHexColor("#151517"))
        assertFalse(AppScreenRules.isHexColor("red"))
    }
}
