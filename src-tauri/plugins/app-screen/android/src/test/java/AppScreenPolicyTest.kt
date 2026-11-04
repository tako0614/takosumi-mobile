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

    @Test
    fun clearTargetsNeverSelectSharedOrAnotherConnection() {
        val cases = JSONObject(listOf(File("../clear-profile-cases.json"), File("clear-profile-cases.json"))
            .first { it.isFile }.readText()).getJSONArray("cases")
        for (index in 0 until cases.length()) {
            val item = cases.getJSONObject(index)
            assertEquals(item.toString(), item.getBoolean("valid"),
                AppScreenProfiles.canClear(item.getString("profile"), item.getString("key")))
        }
    }

    @Test
    fun clearingWebHomeMetadataPreservesOtherProfilesAndLegacyPreferences() {
        val context = org.robolectric.RuntimeEnvironment.getApplication()
        val profile = "11111111-1111-4111-8111-111111111111"
        val otherProfile = "22222222-2222-4222-8222-222222222222"
        val work = ScreenRequest("web-home:$profile:https://sumi.example", "https://sumi.example/", "Work", profile = profile, webHome = true).let { it.copy(scope = it.key, resume = true) }
        val personal = work.copy(key = "web-home:$otherProfile:https://sumi.example", profile = otherProfile).let { it.copy(scope = it.key) }
        val legacy = work.copy(key = "legacy-app", profile = "shared", webHome = false, scope = "legacy-scope")
        val alternate = work.copy(scope = "accepted-alternate-scope")
        for (request in listOf(work, personal, legacy, alternate)) {
            AppScreenResume.record(context, request, "https://sumi.example/editor")
            AppScreenLaunch.remember(context, request)
        }
        assertTrue(AppScreenProfileData.advanceGeneration(context, profile))
        assertTrue(AppScreenResume.clearWebHome(context, profile, work.key))
        assertTrue(AppScreenLaunch.clearWebHome(context, profile, work.key))
        assertEquals(null, AppScreenResume.recall(context, work))
        assertEquals(null, AppScreenResume.recall(context, alternate))
        assertEquals("https://sumi.example/editor", AppScreenResume.recall(context, personal))
        assertEquals("https://sumi.example/editor", AppScreenResume.recall(context, legacy))
    }

    @Test
    fun oldOpaqueResumeScopesCannotReturnAfterTheProfileGenerationChanges() {
        val context = org.robolectric.RuntimeEnvironment.getApplication()
        val profile = "33333333-3333-4333-8333-333333333333"
        val key = "web-home:$profile:https://sumi.example"
        val request = ScreenRequest(key, "https://sumi.example/", "Work", profile = profile,
            webHome = true, scope = "old-alternate-scope", resume = true)
        val preferences = context.getSharedPreferences("com.takosumi.appscreen.resume.v1", android.content.Context.MODE_PRIVATE)
        val id = AppScreenLaunch.digest("${request.scope}\n$key")
        val old = JSONObject().put("url", "https://sumi.example/private-draft").put("t", 1).toString()
        assertTrue(preferences.edit().putString(id, old).commit())
        assertEquals("https://sumi.example/private-draft", AppScreenResume.recall(context, request))
        val before = AppScreenProfileData.generation(context, profile)
        assertTrue(AppScreenProfileData.advanceGeneration(context, profile))
        assertTrue(AppScreenResume.clearWebHome(context, profile, key))
        assertEquals(before + 1L, AppScreenProfileData.generation(context, profile))
        assertEquals(null, AppScreenResume.recall(context, request))
        // Opaque v1 hashes cannot safely be assigned to a profile for physical
        // removal. A tombstone disables their restoration without deleting
        // unrelated accounts' preferences or claiming forensic erasure.
        assertEquals(old, preferences.getString(id, null))
        AppScreenResume.record(context, request, "https://sumi.example/stale-callback", before)
        assertEquals(null, AppScreenResume.recall(context, request))
        AppScreenResume.record(context, request, "https://sumi.example/new-page", before + 1L)
        assertEquals("https://sumi.example/new-page", AppScreenResume.recall(context, request))
    }

    private fun expected(decision: String): NavigationKind = when (decision) {
        "in-app" -> NavigationKind.IN_APP
        "browser-view" -> NavigationKind.BROWSER_VIEW
        "external-confirm" -> NavigationKind.EXTERNAL_CONFIRM
        "block" -> NavigationKind.BLOCK
        else -> throw IllegalArgumentException(decision)
    }

    @Test
    fun profilesUseCanonicalNamesAndRestoreWithoutFallingBackToShared() {
        val file = listOf(java.io.File("../profile-cases.json"), java.io.File("profile-cases.json"))
            .first { it.isFile }
        val cases = org.json.JSONObject(file.readText()).getJSONArray("cases")
        for (index in 0 until cases.length()) {
            val item = cases.getJSONObject(index)
            val profile = item.getString("profile")
            assertEquals(profile, item.getBoolean("valid"), AppScreenProfiles.isValid(profile))
        }
        val profile = "11111111-1111-4111-8111-111111111111"
        val home = ScreenRequest("web-home:$profile:https://sumi.example", "https://sumi.example/", "Work", webHome = true, profile = profile)
        val restored = ScreenRequest.fromJson(home.key, home.toJson())
        assertEquals(profile, restored.profile)
        assertEquals(home.key, restored.key)
        assertEquals(null, ScreenRequest.checkedProfile(profile, webHome = false))
        assertEquals(null, ScreenRequest.checkedProfile("invalid", webHome = true))
        assertEquals("shared", ScreenRequest.checkedProfile(null, webHome = false))
        val legacyKey = "web-home:https://sumi.example"
        val legacy = home.copy(key = legacyKey, profile = "shared").toJson().apply { remove("profile") }
        assertEquals("shared", ScreenRequest.fromJson(legacyKey, legacy).profile)
        for (broken in listOf(
            home.toJson().apply { remove("profile") },
            home.toJson().put("profile", "invalid"),
            home.toJson().put("profile", "22222222-2222-4222-8222-222222222222"),
            home.toJson().apply { remove("webHome") },
        )) {
            try {
                ScreenRequest.fromJson(home.key, broken)
                org.junit.Assert.fail("Malformed isolated identity restored")
            } catch (_: IllegalArgumentException) { }
        }
        val context = org.robolectric.RuntimeEnvironment.getApplication()
        val validIntent = AppScreenLaunch.documentIntent(context, home)
        assertEquals(profile, AppScreenLaunch.parse(context, validIntent)?.profile)
        val missing = AppScreenLaunch.documentIntent(context, home).apply { removeExtra(AppScreenLaunch.EXTRA_PROFILE) }
        assertEquals(null, AppScreenLaunch.parse(context, missing))
        val mismatch = AppScreenLaunch.documentIntent(context, home).apply {
            putExtra(AppScreenLaunch.EXTRA_PROFILE, "22222222-2222-4222-8222-222222222222")
        }
        assertEquals(null, AppScreenLaunch.parse(context, mismatch))
        AppScreenLaunch.remember(context, home)
        val conflictingIdentity = android.content.Intent(android.content.Intent.ACTION_VIEW).apply {
            data = AppScreenLaunch.documentUri(home.key)
            putExtra(AppScreenLaunch.EXTRA_KEY, "web-home:22222222-2222-4222-8222-222222222222:https://sumi.example")
        }
        assertEquals(null, AppScreenLaunch.parse(context, conflictingIdentity))
    }

    @Test
    fun webHomeKeepsWebNavigationAndSurvivesTaskRestoration() {
        val home = ScreenRequest("web-home:https://sumi.example", "https://sumi.example/", "Sumi", webHome = true)
        assertEquals(null, home.origins)
        assertTrue(AppScreenPolicy.isInScreen("https://sumi.example/apps", home.origins))
        assertTrue(AppScreenPolicy.isInScreen("https://app.example/inbox", home.origins))
        assertEquals(false, AppScreenPolicy.isInScreen("https://ipc.localhost/", home.origins))
        val restored = ScreenRequest.fromJson(home.key, home.toJson())
        assertTrue(restored.webHome)
        assertEquals(null, restored.origins)
        val legacy = ScreenRequest("old", "https://app.example/", "App", accountsOrigins = setOf("https://sumi.example"))
        assertEquals(false, AppScreenPolicy.isInScreen("https://sumi.example/apps", legacy.origins))
        for (url in listOf("https://sumi.example/?token=secret", "https://sumi.example/#secret", "https://sumi.example/apps", "https://ipc.localhost/")) {
            assertEquals(url, false, ScreenRequest.isWebHomeUrl(url))
        }
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
