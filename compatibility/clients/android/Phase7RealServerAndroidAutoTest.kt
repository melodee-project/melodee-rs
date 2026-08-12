package com.melodee.autoplayer

import android.content.ComponentName
import android.os.Bundle
import android.os.SystemClock
import android.support.v4.media.MediaBrowserCompat
import android.support.v4.media.session.MediaControllerCompat
import androidx.test.core.app.ApplicationProvider
import androidx.test.ext.junit.runners.AndroidJUnit4
import androidx.test.platform.app.InstrumentationRegistry
import com.melodee.autoplayer.service.MusicService
import com.melodee.autoplayer.util.AuthenticationHelper
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test
import org.junit.runner.RunWith

/**
 * Reusable Phase 7 probe for the shipping Android Auto MediaBrowser/MediaSession path.
 *
 * The runner copies this file into a clean pinned client checkout and removes it on exit. The test
 * talks only to the disposable Rust fixture supplied in instrumentation arguments.
 */
@RunWith(AndroidJUnit4::class)
class Phase7RealServerAndroidAutoTest {
    private val instrumentation = InstrumentationRegistry.getInstrumentation()
    private val context = ApplicationProvider.getApplicationContext<android.content.Context>()
    private val arguments = InstrumentationRegistry.getArguments()
    private lateinit var browser: MediaBrowserCompat

    @Before
    fun authenticateAndConnect() {
        val baseUrl = requiredArgument("baseUrl").trimEnd('/') + "/"
        val email = requiredArgument("email")
        val password = requiredArgument("password")
        val login = CountDownLatch(1)
        val error = AtomicReference<String?>()
        instrumentation.runOnMainSync {
            AuthenticationHelper(context).run {
                logout()
                login(baseUrl, email, password, { login.countDown() }) {
                    error.set(it)
                    login.countDown()
                }
            }
        }
        assertTrue("login timed out", login.await(20, TimeUnit.SECONDS))
        assertEquals("login failed", null, error.get())

        val connected = CountDownLatch(1)
        val connectionError = AtomicReference<String?>()
        browser = MediaBrowserCompat(
            context,
            ComponentName(context, MusicService::class.java),
            object : MediaBrowserCompat.ConnectionCallback() {
                override fun onConnected() = connected.countDown()
                override fun onConnectionFailed() {
                    connectionError.set("connection failed")
                    connected.countDown()
                }
                override fun onConnectionSuspended() {
                    connectionError.set("connection suspended")
                    connected.countDown()
                }
            },
            Bundle().apply { putBoolean("android.media.browse.extra.RECENT", true) },
        )
        instrumentation.runOnMainSync { browser.connect() }
        assertTrue("MediaBrowser connection timed out", connected.await(10, TimeUnit.SECONDS))
        assertEquals(null, connectionError.get())
        assertTrue(browser.isConnected)
    }

    @After
    fun disconnect() {
        if (::browser.isInitialized) instrumentation.runOnMainSync { browser.disconnect() }
    }

    @Test
    fun browseSearchPlaybackSeekAndQueueControlsUseTheRustServer() {
        val root = children("root")
        assertTrue(root.any { it.description.title?.toString() == "Playlists" })
        assertTrue(root.any { it.description.title?.toString() == "Current Queue" })

        val playlists = children("playlists")
        val playlist = playlists.single { it.description.title?.toString() == "Phase Seven Client Playlist" }
        val tracks = children(requireNotNull(playlist.mediaId))
        assertEquals(2, tracks.size)
        assertEquals("Phase Seven Playback Track", tracks[0].description.title?.toString())
        assertEquals("Phase Seven Next Track", tracks[1].description.title?.toString())

        val search = search("Phase Seven Playback Track")
        assertTrue(search.any { it.description.title?.toString() == "Phase Seven Playback Track" })

        val controller = MediaControllerCompat(context, browser.sessionToken)
        controller.transportControls.playFromMediaId(tracks[0].mediaId, tracks[0].description.extras)
        awaitTitle(controller, "Phase Seven Playback Track")
        controller.transportControls.skipToNext()
        awaitTitle(controller, "Phase Seven Next Track")
        controller.transportControls.skipToPrevious()
        awaitTitle(controller, "Phase Seven Playback Track")
        controller.transportControls.seekTo(29_500)
        awaitTitle(controller, "Phase Seven Next Track", 12_000)

        controller.transportControls.playFromSearch("Phase Seven Playback Track", Bundle.EMPTY)
        awaitTitle(controller, "Phase Seven Playback Track")
        assertTrue(AuthenticationHelper(context).isAuthenticated())
    }

    private fun children(parentId: String): List<MediaBrowserCompat.MediaItem> {
        val result = AtomicReference<List<MediaBrowserCompat.MediaItem>>()
        val error = AtomicReference<String?>()
        val latch = CountDownLatch(1)
        val callback = object : MediaBrowserCompat.SubscriptionCallback() {
            override fun onChildrenLoaded(parentId: String, children: MutableList<MediaBrowserCompat.MediaItem>) {
                result.set(children.toList())
                latch.countDown()
            }
            override fun onError(parentId: String) {
                error.set("browse failed for $parentId")
                latch.countDown()
            }
        }
        instrumentation.runOnMainSync { browser.subscribe(parentId, callback) }
        assertTrue("browse timed out for $parentId", latch.await(15, TimeUnit.SECONDS))
        instrumentation.runOnMainSync { browser.unsubscribe(parentId, callback) }
        assertEquals(null, error.get())
        return requireNotNull(result.get())
    }

    private fun search(query: String): List<MediaBrowserCompat.MediaItem> {
        val result = AtomicReference<List<MediaBrowserCompat.MediaItem>>()
        val latch = CountDownLatch(1)
        instrumentation.runOnMainSync {
            browser.search(query, Bundle.EMPTY, object : MediaBrowserCompat.SearchCallback() {
                override fun onSearchResult(
                    query: String,
                    extras: Bundle,
                    items: MutableList<MediaBrowserCompat.MediaItem>,
                ) {
                    result.set(items.toList())
                    latch.countDown()
                }
                override fun onError(query: String, extras: Bundle) = latch.countDown()
            })
        }
        assertTrue("search timed out", latch.await(15, TimeUnit.SECONDS))
        return requireNotNull(result.get())
    }

    private fun awaitTitle(
        controller: MediaControllerCompat,
        expected: String,
        timeoutMs: Long = 8_000,
    ) {
        val deadline = SystemClock.elapsedRealtime() + timeoutMs
        while (SystemClock.elapsedRealtime() < deadline) {
            if (controller.metadata?.description?.title?.toString() == expected) return
            SystemClock.sleep(100)
        }
        assertEquals(expected, controller.metadata?.description?.title?.toString())
    }

    private fun requiredArgument(name: String): String =
        requireNotNull(arguments.getString(name)) { "missing instrumentation argument $name" }
}
