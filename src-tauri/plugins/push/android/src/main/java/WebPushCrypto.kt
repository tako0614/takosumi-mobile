package com.takosumi.push

import java.math.BigInteger
import java.security.AlgorithmParameters
import java.security.GeneralSecurityException
import java.security.KeyFactory
import java.security.KeyPairGenerator
import java.security.PrivateKey
import java.security.SecureRandom
import java.security.interfaces.ECPublicKey
import java.security.spec.ECGenParameterSpec
import java.security.spec.ECParameterSpec
import java.security.spec.ECPoint
import java.security.spec.ECPrivateKeySpec
import java.security.spec.ECPublicKeySpec
import java.security.spec.PKCS8EncodedKeySpec
import javax.crypto.Cipher
import javax.crypto.KeyAgreement
import javax.crypto.Mac
import javax.crypto.spec.GCMParameterSpec
import javax.crypto.spec.SecretKeySpec

/** A message this device cannot read; the caller shows generic text instead. */
class WebPushDecryptException(message: String) : Exception(message)

/**
 * The receiving side of RFC 8291 (Message Encryption for Web Push) over
 * RFC 8188 `aes128gcm` (one record), as an instance sends it to the push relay. Plain JVM
 * code (java.security / javax.crypto only), so the RFC 8291 Appendix A vector
 * and the shared `web-push-cases.json` run as ordinary unit tests.
 */
object WebPushCrypto {
    const val PUBLIC_KEY_BYTES = 65
    const val AUTH_SECRET_BYTES = 16
    /** The relay refuses bodies above 2,800 bytes; anything far larger is not ours. */
    const val MAX_BODY_BYTES = 4096

    private const val SALT_BYTES = 16
    private const val HEADER_BYTES = SALT_BYTES + 4 + 1
    private const val TAG_BYTES = 16
    private const val MIN_RECORD_SIZE = 18L
    private const val KEY_BYTES = 16
    private const val NONCE_BYTES = 12

    // P-256: y^2 = x^3 - 3x + b over GF(p).
    private val P = BigInteger("ffffffff00000001000000000000000000000000ffffffffffffffffffffffff", 16)
    private val B = BigInteger("5ac635d8aa3a93e7b3ebbd55769886bc651d06b0cc53b0f63bce3c3e27d2604b", 16)
    private val THREE = BigInteger.valueOf(3)

    private val curve: ECParameterSpec by lazy {
        val parameters = AlgorithmParameters.getInstance("EC")
        parameters.init(ECGenParameterSpec("secp256r1"))
        parameters.getParameterSpec(ECParameterSpec::class.java)
    }

    /** A new subscription's key pair and `auth` secret. */
    class KeyMaterial(
        val publicKey: ByteArray,
        val privateKeyPkcs8: ByteArray,
        val authSecret: ByteArray,
    )

    fun generate(random: SecureRandom = SecureRandom()): KeyMaterial {
        val generator = KeyPairGenerator.getInstance("EC")
        generator.initialize(ECGenParameterSpec("secp256r1"), random)
        val pair = generator.generateKeyPair()
        val auth = ByteArray(AUTH_SECRET_BYTES)
        random.nextBytes(auth)
        return KeyMaterial(encodePublic(pair.public as ECPublicKey), pair.private.encoded, auth)
    }

    /** The uncompressed point (0x04 || X || Y) Web Push calls `p256dh`. */
    fun encodePublic(key: ECPublicKey): ByteArray {
        val out = ByteArray(PUBLIC_KEY_BYTES)
        out[0] = 0x04
        writeFixed(key.w.affineX, out, 1)
        writeFixed(key.w.affineY, out, 33)
        return out
    }

    fun privateFromPkcs8(bytes: ByteArray): PrivateKey =
        KeyFactory.getInstance("EC").generatePrivate(PKCS8EncodedKeySpec(bytes))

    /** A raw 32-byte private scalar, as the RFC 8291 test vector gives it. */
    fun privateFromScalar(d: ByteArray): PrivateKey =
        KeyFactory.getInstance("EC").generatePrivate(ECPrivateKeySpec(BigInteger(1, d), curve))

    /** Parses an uncompressed P-256 point and refuses one that is not on the curve. */
    fun publicFromUncompressed(bytes: ByteArray): ECPublicKey {
        if (bytes.size != PUBLIC_KEY_BYTES || bytes[0] != 0x04.toByte()) {
            throw WebPushDecryptException("the sender key is not an uncompressed P-256 point")
        }
        val x = BigInteger(1, bytes.copyOfRange(1, 33))
        val y = BigInteger(1, bytes.copyOfRange(33, 65))
        if (x >= P || y >= P) throw WebPushDecryptException("the sender key is out of range")
        val left = y.multiply(y).mod(P)
        val right = x.pow(3).subtract(x.multiply(THREE)).add(B).mod(P)
        if (left != right) throw WebPushDecryptException("the sender key is not on P-256")
        return try {
            KeyFactory.getInstance("EC").generatePublic(ECPublicKeySpec(ECPoint(x, y), curve)) as ECPublicKey
        } catch (error: GeneralSecurityException) {
            throw WebPushDecryptException("the sender key is unusable")
        }
    }

    /**
     * Decrypts one Web Push message body: the RFC 8188 header (salt, record
     * size, the sender's public key as key id) and its records.
     */
    fun decrypt(
        body: ByteArray,
        uaPrivate: PrivateKey,
        uaPublic: ByteArray,
        authSecret: ByteArray,
    ): ByteArray {
        if (body.size > MAX_BODY_BYTES) throw WebPushDecryptException("the message is too large")
        if (authSecret.size != AUTH_SECRET_BYTES) throw WebPushDecryptException("auth must be 16 bytes")
        if (uaPublic.size != PUBLIC_KEY_BYTES) throw WebPushDecryptException("the subscription key is invalid")
        if (body.size < HEADER_BYTES) throw WebPushDecryptException("the message has no header")
        val salt = body.copyOfRange(0, SALT_BYTES)
        val recordSize = readUint32(body, SALT_BYTES)
        val idLength = body[SALT_BYTES + 4].toInt() and 0xff
        if (idLength != PUBLIC_KEY_BYTES) throw WebPushDecryptException("the key id is not a P-256 key")
        if (recordSize < MIN_RECORD_SIZE) throw WebPushDecryptException("the record size is invalid")
        val keyEnd = HEADER_BYTES + idLength
        if (body.size <= keyEnd + TAG_BYTES) throw WebPushDecryptException("the message has no records")
        val asPublic = body.copyOfRange(HEADER_BYTES, keyEnd)

        val agreement = KeyAgreement.getInstance("ECDH")
        agreement.init(uaPrivate)
        agreement.doPhase(publicFromUncompressed(asPublic), true)
        val ecdhSecret = agreement.generateSecret()

        // RFC 8291 section 3.3 / 3.4.
        val keyInfo = ascii("WebPush: info\u0000") + uaPublic + asPublic
        val ikm = expand(hmac(authSecret, ecdhSecret), keyInfo, 32)
        val prk = hmac(salt, ikm)
        val cek = expand(prk, ascii("Content-Encoding: aes128gcm\u0000"), KEY_BYTES)
        val nonce = expand(prk, ascii("Content-Encoding: nonce\u0000"), NONCE_BYTES)

        // RFC 8291 section 4: a Web Push message is one record. The relay
        // refuses anything else, so a second record is never ours either.
        val record = body.copyOfRange(keyEnd, body.size)
        if (record.size > recordSize) throw WebPushDecryptException("the message has more than one record")
        val plain = try {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(Cipher.DECRYPT_MODE, SecretKeySpec(cek, "AES"), GCMParameterSpec(TAG_BYTES * 8, nonce))
            cipher.doFinal(record)
        } catch (error: GeneralSecurityException) {
            throw WebPushDecryptException("the record does not authenticate")
        }
        return unpad(plain)
    }

    /** RFC 8188 section 2: data, the last-record delimiter 0x02, then zeros. */
    private fun unpad(plain: ByteArray): ByteArray {
        var index = plain.size - 1
        while (index >= 0 && plain[index] == 0.toByte()) index -= 1
        if (index < 0 || plain[index] != 2.toByte()) {
            throw WebPushDecryptException("the record has no last-record delimiter")
        }
        return plain.copyOfRange(0, index)
    }

    private fun hmac(key: ByteArray, data: ByteArray): ByteArray {
        val mac = Mac.getInstance("HmacSHA256")
        mac.init(SecretKeySpec(key, "HmacSHA256"))
        return mac.doFinal(data)
    }

    /** HKDF-Expand for at most one block (L <= 32). */
    private fun expand(prk: ByteArray, info: ByteArray, length: Int): ByteArray =
        hmac(prk, info + byteArrayOf(1)).copyOf(length)

    private fun readUint32(bytes: ByteArray, offset: Int): Long =
        ((bytes[offset].toLong() and 0xff) shl 24) or
            ((bytes[offset + 1].toLong() and 0xff) shl 16) or
            ((bytes[offset + 2].toLong() and 0xff) shl 8) or
            (bytes[offset + 3].toLong() and 0xff)

    private fun writeFixed(value: BigInteger, out: ByteArray, offset: Int) {
        val bytes = value.toByteArray()
        val trimmed = if (bytes.size > 32) bytes.copyOfRange(bytes.size - 32, bytes.size) else bytes
        System.arraycopy(trimmed, 0, out, offset + 32 - trimmed.size, trimmed.size)
    }

    private fun ascii(text: String): ByteArray = text.toByteArray(Charsets.US_ASCII)
}
