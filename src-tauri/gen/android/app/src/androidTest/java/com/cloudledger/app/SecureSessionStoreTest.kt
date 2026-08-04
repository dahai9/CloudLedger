package com.cloudledger.app

import android.content.Context
import android.content.pm.ApplicationInfo
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import java.io.File
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class SecureSessionStoreTest {
    private lateinit var context: Context

    @Before
    fun setUp() {
        context = InstrumentationRegistry.getInstrumentation().targetContext
        SecureSessionStore.clear(context)
    }

    @After
    fun tearDown() {
        SecureSessionStore.clear(context)
    }

    @Test
    fun keystoreCiphertextPersistsAndLogoutClearsIt() {
        val payload = """{"refreshToken":"secret","installationId":"device"}"""
        SecureSessionStore.store(context, payload)
        assertEquals(payload, SecureSessionStore.load(context))

        SecureSessionStore.clear(context)
        assertNull(SecureSessionStore.load(context))
        assertFalse(sessionFile().exists())
    }

    @Test
    fun copiedCiphertextCannotBeDecryptedWithoutOriginalKeystoreKey() {
        SecureSessionStore.store(context, "device-bound-secret")
        val copiedCiphertext = sessionFile().readText(Charsets.UTF_8)

        SecureSessionStore.clear(context)
        sessionFile().writeText(copiedCiphertext, Charsets.UTF_8)

        assertNull(SecureSessionStore.load(context))
        assertFalse(sessionFile().exists())
    }

    @Test
    fun releaseManifestDisablesCleartextTraffic() {
        if (!BuildConfig.DEBUG) {
            val permitsCleartext =
                context.applicationInfo.flags and ApplicationInfo.FLAG_USES_CLEARTEXT_TRAFFIC != 0
            assertFalse(permitsCleartext)
        }
    }

    private fun sessionFile() =
        File(context.noBackupFilesDir, "cloudledger-secure-session.v1")
}
