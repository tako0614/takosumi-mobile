package com.takosumi.push

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertThrows
import org.junit.Test
import java.util.Base64

/** RFC 8291 decryption on the plain JVM (no Android classes involved). */
class WebPushCryptoTest {
    private fun b64(text: String): ByteArray = Base64.getUrlDecoder().decode(text)

    // RFC 8291, Appendix A.
    private val uaPublic = b64("BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4")
    private val uaPrivate = b64("q1dXpw3UpT5VOmu_cf_v6ih07Aems3njxI-JWgLcM94")
    private val authSecret = b64("BTBZMqHH6r4Tts7J_aSIgg")
    private val body = b64(
        "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN",
    )

    private fun decrypt(message: ByteArray, auth: ByteArray = authSecret): ByteArray =
        WebPushCrypto.decrypt(message, WebPushCrypto.privateFromScalar(uaPrivate), uaPublic, auth)

    @Test
    fun decryptsTheRfc8291AppendixAMessage() {
        assertEquals("When I grow up, I want to be a watermelon", String(decrypt(body), Charsets.UTF_8))
    }

    @Test
    fun aPkcs8KeyDecryptsLikeTheRawScalar() {
        val pkcs8 = WebPushCrypto.privateFromScalar(uaPrivate).encoded
        val plain = WebPushCrypto.decrypt(body, WebPushCrypto.privateFromPkcs8(pkcs8), uaPublic, authSecret)
        assertEquals("When I grow up, I want to be a watermelon", String(plain, Charsets.UTF_8))
    }

    @Test
    fun aGeneratedSubscriptionHasAnUncompressedPointAndSixteenByteAuth() {
        val keys = WebPushCrypto.generate()
        assertEquals(65, keys.publicKey.size)
        assertEquals(0x04.toByte(), keys.publicKey[0])
        assertEquals(16, keys.authSecret.size)
        // The point is on the curve and survives a round trip.
        val parsed = WebPushCrypto.publicFromUncompressed(keys.publicKey)
        assertArrayEquals(keys.publicKey, WebPushCrypto.encodePublic(parsed))
        assertNotNull(WebPushCrypto.privateFromPkcs8(keys.privateKeyPkcs8))
    }

    @Test
    fun refusesAWrongAuthSecretAndATamperedRecord() {
        assertThrows(WebPushDecryptException::class.java) { decrypt(body, ByteArray(16)) }
        val tampered = body.copyOf()
        tampered[tampered.size - 1] = (tampered[tampered.size - 1].toInt() xor 1).toByte()
        assertThrows(WebPushDecryptException::class.java) { decrypt(tampered) }
    }

    @Test
    fun refusesMalformedHeaders() {
        // Truncated before the records.
        assertThrows(WebPushDecryptException::class.java) { decrypt(body.copyOf(21 + 65)) }
        // A key id that is not a 65-byte P-256 point.
        val shortKeyId = body.copyOf()
        shortKeyId[20] = 64
        assertThrows(WebPushDecryptException::class.java) { decrypt(shortKeyId) }
        // A record size below the RFC 8188 minimum.
        val tinyRecord = body.copyOf()
        tinyRecord[16] = 0
        tinyRecord[17] = 0
        tinyRecord[18] = 0
        tinyRecord[19] = 17
        assertThrows(WebPushDecryptException::class.java) { decrypt(tinyRecord) }
        // A record size smaller than the ciphertext: a second record.
        val twoRecords = body.copyOf()
        twoRecords[18] = 0
        twoRecords[19] = 40
        assertThrows(WebPushDecryptException::class.java) { decrypt(twoRecords) }
        // Larger than any relay message.
        assertThrows(WebPushDecryptException::class.java) { decrypt(body + ByteArray(WebPushCrypto.MAX_BODY_BYTES)) }
        // A wrong auth length.
        assertThrows(WebPushDecryptException::class.java) { decrypt(body, ByteArray(15)) }
    }

    @Test
    fun refusesASenderKeyOffTheCurve() {
        val offCurve = body.copyOf()
        // Change Y of the sender key (bytes 21..85): no longer on P-256.
        offCurve[21 + 64] = (offCurve[21 + 64].toInt() xor 1).toByte()
        assertThrows(WebPushDecryptException::class.java) { decrypt(offCurve) }
        val compressed = ByteArray(65).also { it[0] = 0x02 }
        assertThrows(WebPushDecryptException::class.java) { WebPushCrypto.publicFromUncompressed(compressed) }
    }
}
