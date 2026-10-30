package com.takosumi.appscreen

import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import java.io.File

/**
 * Runs [AppScreenPolicy.decide] — with Android's own `Uri` parser, through
 * Robolectric — against every case of `navigation-policy-cases.json`, the
 * table `src/url-policy.ts` is tested with.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], manifest = Config.NONE)
class AppScreenPolicyTest {
    private fun fixture(): JSONObject {
        val file = listOf(File("../navigation-policy-cases.json"), File("navigation-policy-cases.json"))
            .first { it.isFile }
        return JSONObject(file.readText())
    }

    private fun expected(decision: String): NavigationKind = when (decision) {
        "in-app" -> NavigationKind.IN_APP
        "browser-view" -> NavigationKind.BROWSER_VIEW
        "external-confirm" -> NavigationKind.EXTERNAL_CONFIRM
        "block" -> NavigationKind.BLOCK
        else -> throw IllegalArgumentException(decision)
    }

    @Test
    fun everySharedCaseMatchesTheAndroidPolicy() {
        val cases = fixture().getJSONArray("cases")
        assertTrue(cases.length() > 80)
        val mismatches = mutableListOf<String>()
        for (index in 0 until cases.length()) {
            val case = cases.getJSONObject(index)
            val url = case.getString("url")
            val mainFrame = case.getBoolean("mainFrame")
            val origins = case.optString("appOrigin").ifEmpty { null }?.let { appOrigin ->
                val accounts = case.optJSONArray("accountsOrigins")
                ScreenOrigins(appOrigin, List(accounts?.length() ?: 0) { accounts!!.getString(it) }.toSet())
            }
            val kind = AppScreenPolicy.decide(url, mainFrame, origins).kind
            val want = expected(case.getString("decision"))
            if (kind != want) mismatches.add("$url mainFrame=$mainFrame app=${origins?.appOrigin}: $kind != $want")
        }
        assertEquals(mismatches.joinToString("\n"), 0, mismatches.size)
    }

    @Test
    fun theScreenShowsItsLaunchPageOnlyWhenThePolicyDoes() {
        val origins = ScreenOrigins("https://app.example", setOf("https://takosumi.example"))
        assertTrue(AppScreenPolicy.isInScreen("https://app.example/inbox", origins))
        assertTrue(AppScreenPolicy.isInScreen("https://takosumi.example/oauth/authorize?x=1", origins))
        assertEquals(false, AppScreenPolicy.isInScreen("https://takosumi.example/apps", origins))
        assertEquals(false, AppScreenPolicy.isInScreen("http://app.example/", origins))
    }
}
