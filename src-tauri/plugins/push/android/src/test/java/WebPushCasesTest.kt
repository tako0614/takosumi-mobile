package com.takosumi.push

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Test
import java.io.File
import java.util.Base64

/**
 * Every case of `web-push-cases.json` (messages encrypted by the relay's
 * standards Web Push tool, the same table `tests/push-cases.test.ts` reads):
 * decryption with the subscription's key, then the Declarative Web Push parse
 * that decides what the notification shows and opens. Plain JVM with the real
 * org.json, so it also runs in CI without Android (`jvm-tests`).
 */
class WebPushCasesTest {
    private fun fixture(): JSONObject {
        val file = listOf(File("../web-push-cases.json"), File("web-push-cases.json")).first { it.isFile }
        return JSONObject(file.readText())
    }

    private fun b64(text: String): ByteArray = Base64.getUrlDecoder().decode(text)

    @Test
    fun everyCaseDecryptsAndShowsAsTheTableSays() {
        val table = fixture()
        val subscription = table.getJSONObject("subscription")
        val uaPublic = b64(subscription.getString("uaPublic"))
        val uaPrivate = WebPushCrypto.privateFromScalar(b64(subscription.getString("uaPrivate")))
        val originsJson = subscription.getJSONArray("origins")
        val origins = (0 until originsJson.length()).map { originsJson.getString(it) }
        val cases = table.getJSONArray("cases")
        assertTrue(cases.length() >= 10)
        for (index in 0 until cases.length()) {
            val case = cases.getJSONObject(index)
            val name = case.getString("name")
            val auth = b64(case.optString("authSecret", subscription.getString("authSecret")))
            val plaintext = try {
                WebPushCrypto.decrypt(b64(case.getString("body")), uaPrivate, uaPublic, auth)
            } catch (error: WebPushDecryptException) {
                if (case.getBoolean("decrypts")) fail("$name: ${error.message}")
                continue
            }
            if (!case.getBoolean("decrypts")) fail("$name: decrypted")
            assertEquals(name, case.getString("plaintext"), String(plaintext, Charsets.UTF_8))
            val parsed = DeclarativePush.parse(plaintext, origins)
            val expected = case.optJSONObject("notification")
            if (expected == null) {
                assertNull(name, parsed)
                continue
            }
            if (parsed == null) fail("$name: not shown")
            parsed!!
            assertEquals(name, expected.getString("title"), parsed.title)
            assertEquals(name, expected.optStringOrNull("body"), parsed.body)
            assertEquals(name, expected.optStringOrNull("navigate"), parsed.navigate)
            assertEquals(name, expected.optStringOrNull("lang"), parsed.lang)
            assertEquals(name, expected.optStringOrNull("tag"), parsed.tag)
        }
    }

    @Test
    fun theRfcVectorIsInTheTable() {
        val rfc = fixture().getJSONObject("rfc8291")
        val plain = WebPushCrypto.decrypt(
            b64(rfc.getString("body")),
            WebPushCrypto.privateFromScalar(b64(rfc.getString("uaPrivate"))),
            b64(rfc.getString("uaPublic")),
            b64(rfc.getString("authSecret")),
        )
        assertEquals(rfc.getString("plaintext"), String(plain, Charsets.UTF_8))
    }

    @Test
    fun longTextIsShortenedAndInvalidUtf8IsRefused() {
        val origins = listOf("https://takosumi.example")
        val long = "あ".repeat(500)
        val message = """{"web_push":8030,"notification":{"title":"$long","body":"$long","navigate":"https://takosumi.example/"}}"""
        val parsed = DeclarativePush.parse(message.toByteArray(Charsets.UTF_8), origins)!!
        assertEquals(120, parsed.title.codePointCount(0, parsed.title.length))
        assertTrue(parsed.title.endsWith("…"))
        assertEquals(500, parsed.body!!.codePointCount(0, parsed.body!!.length))
        assertNull(DeclarativePush.parse(byteArrayOf(0x7b, 0xff.toByte(), 0x7d), origins))
    }

    @Test
    fun originsCompareWithTheDefaultPortDroppedAndCaseFolded() {
        val origins = listOf("https://takosumi.example", "http://localhost:8787")
        assertEquals(
            "https://TAKOSUMI.example:443/runs/1",
            DeclarativePush.allowedUrl("https://TAKOSUMI.example:443/runs/1", origins),
        )
        assertEquals("http://localhost:8787/x", DeclarativePush.allowedUrl("http://localhost:8787/x", origins))
        assertNull(DeclarativePush.allowedUrl("https://takosumi.example:8443/", origins))
        assertNull(DeclarativePush.allowedUrl("javascript:alert(1)", origins))
        assertNull(DeclarativePush.allowedUrl("/runs/1", origins))
        assertNull(DeclarativePush.allowedUrl("takosumi://app?url=https://takosumi.example/", origins))
    }

    private fun JSONObject.optStringOrNull(key: String): String? = if (has(key)) getString(key) else null
}
