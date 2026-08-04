package com.cloudledger.app

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.annotation.Keep
import java.io.File
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@Keep
object SecureSessionStore {
    private const val KEY_ALIAS = "cloudledger.refresh.v1"
    private const val STORE_FILE = "cloudledger-secure-session.v1"
    private const val TRANSFORMATION = "AES/GCM/NoPadding"

    @JvmStatic
    @Keep
    fun store(context: Context, payload: String) {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, getOrCreateKey())
        cipher.updateAAD(context.packageName.toByteArray(Charsets.UTF_8))
        val encrypted = cipher.doFinal(payload.toByteArray(Charsets.UTF_8))
        val encoded = Base64.encodeToString(cipher.iv + encrypted, Base64.NO_WRAP)
        val target = sessionFile(context)
        val temporary = File(target.parentFile, "${target.name}.tmp")
        temporary.writeText(encoded, Charsets.UTF_8)
        if (!temporary.renameTo(target)) {
            temporary.delete()
            throw IllegalStateException("failed to persist secure session")
        }
    }

    @JvmStatic
    @Keep
    fun load(context: Context): String? {
        val target = sessionFile(context)
        if (!target.exists()) return null
        return try {
            val encrypted = Base64.decode(target.readText(Charsets.UTF_8), Base64.NO_WRAP)
            require(encrypted.size > 12) { "invalid secure session" }
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(
                Cipher.DECRYPT_MODE,
                getOrCreateKey(),
                GCMParameterSpec(128, encrypted.copyOfRange(0, 12)),
            )
            cipher.updateAAD(context.packageName.toByteArray(Charsets.UTF_8))
            String(cipher.doFinal(encrypted.copyOfRange(12, encrypted.size)), Charsets.UTF_8)
        } catch (_: Exception) {
            target.delete()
            null
        }
    }

    @JvmStatic
    @Keep
    fun clear(context: Context) {
        sessionFile(context).delete()
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (keyStore.containsAlias(KEY_ALIAS)) keyStore.deleteEntry(KEY_ALIAS)
    }

    private fun sessionFile(context: Context) = File(context.noBackupFilesDir, STORE_FILE)

    private fun getOrCreateKey(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setUserAuthenticationRequired(false)
                .setRandomizedEncryptionRequired(true)
                .build(),
        )
        return generator.generateKey()
    }
}
