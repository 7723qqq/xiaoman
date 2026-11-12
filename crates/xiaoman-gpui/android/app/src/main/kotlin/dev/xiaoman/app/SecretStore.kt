package dev.xiaoman.app

import android.content.Context
import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The Keystore half of the settings store: what `EncryptedSharedPreferences` does in the Kotlin app.
 *
 * One AES-256-GCM key in the Android Keystore encrypts every value; the ciphertext lives in a plain
 * preferences file, so the keys are unreadable without the hardware-backed key. Rust reaches this
 * through `NotifierBridge.{read,write,remove}Secret`, one JSON value per name, and never writes the
 * API keys to `settings-secrets.json`.
 */
object SecretStore {
    private const val TAG = "Xiaoman"
    private const val FILE = "settings-secrets"
    private const val KEY_ALIAS = "xiaoman-settings-secrets"
    private const val GCM_TAG_BITS = 128
    private const val IV_BYTES = 12

    private var prefs: SharedPreferences? = null

    fun init(context: Context) {
        prefs = context.applicationContext.getSharedPreferences(FILE, Context.MODE_PRIVATE)
    }

    fun read(name: String): String? {
        val stored = prefs?.getString(name, null) ?: return null
        return runCatching { decrypt(stored) }.getOrElse { error ->
            // A keystore entry lost after a restore or an OS update: drop the value so the user can
            // type the key again, like the Kotlin store resets an undecryptable secret file.
            Log.w(TAG, "secrets: cannot decrypt $name (${error.javaClass.simpleName}); dropping it")
            prefs?.edit()?.remove(name)?.apply()
            null
        }
    }

    /** True when the value was encrypted and committed; false when the Keystore refused. */
    fun write(name: String, value: String): Boolean {
        val encrypted = runCatching { encrypt(value) }.getOrElse { error ->
            Log.w(TAG, "secrets: cannot encrypt $name: ${error.message}")
            return false
        }
        val store = prefs ?: return false
        return store.edit().putString(name, encrypted).commit()
    }

    fun remove(name: String) {
        prefs?.edit()?.remove(name)?.apply()
    }

    @Synchronized
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build(),
        )
        return generator.generateKey()
    }

    private fun encrypt(plain: String): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val body = cipher.doFinal(plain.toByteArray(Charsets.UTF_8))
        return Base64.encodeToString(cipher.iv + body, Base64.NO_WRAP)
    }

    private fun decrypt(stored: String): String {
        val bytes = Base64.decode(stored, Base64.NO_WRAP)
        val iv = bytes.copyOfRange(0, IV_BYTES)
        val body = bytes.copyOfRange(IV_BYTES, bytes.size)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(GCM_TAG_BITS, iv))
        return String(cipher.doFinal(body), Charsets.UTF_8)
    }
}
