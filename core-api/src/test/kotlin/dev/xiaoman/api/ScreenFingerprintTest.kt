package dev.xiaoman.api

import kotlin.test.Test
import kotlin.test.assertEquals

class ScreenFingerprintTest {
    /** The same constant `crates/xiaoman-api/src/device.rs` pins: Java hashCode -584996425. */
    @Test
    fun fingerprintPinsTheSharedGolden() {
        val screen = ScreenState(
            screenshotPng = null,
            widthPx = 1080,
            heightPx = 2400,
            packageName = "com.example",
            activityName = "Main",
            nodes = listOf(
                UiNode(
                    id = 0,
                    text = "确认",
                    bounds = Bounds(0, 0, 200, 100),
                    clickable = true,
                ),
            ),
        )
        assertEquals("-22de5649", screen.fingerprint())
    }

    @Test
    fun negativeHashesKeepTheMinusSign() {
        // "-1".hashCode() is 1536; a hash of exactly Int.MIN_VALUE must render its magnitude:
        // Integer.toString(Int.MIN_VALUE, 16) == "-80000000". The Rust side pins the same rule.
        assertEquals("-80000000", Int.MIN_VALUE.toString(16))
    }
}
