package dev.locationlab.helper

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test

class GeoTest {
    private val tokyo = LatLng(35.681236, 139.767125)
    private val shinjuku = LatLng(35.690921, 139.700258)

    @Test
    fun rejectsOutOfRangeCoordinates() {
        assertThrows(IllegalArgumentException::class.java) { LatLng(90.1, 0.0) }
        assertThrows(IllegalArgumentException::class.java) { LatLng(0.0, -180.1) }
        assertThrows(IllegalArgumentException::class.java) { LatLng(Double.NaN, 0.0) }
    }

    @Test
    fun parsesCoordinates() {
        assertEquals(tokyo, LatLng.parse(" 35.681236 , 139.767125 "))
        assertEquals(listOf(tokyo, shinjuku), LatLng.parseList("35.681236,139.767125;35.690921,139.700258;"))
        assertThrows(IllegalArgumentException::class.java) { LatLng.parse("35.6") }
        assertThrows(IllegalArgumentException::class.java) { LatLng.parse("abc,139") }
        assertThrows(IllegalArgumentException::class.java) { LatLng.parse("91,139") }
    }

    @Test
    fun distanceTokyoToShinjuku() {
        // 東京駅〜新宿駅の直線距離は約 6.1 km
        assertEquals(6_130.0, Geo.distanceMeters(tokyo, shinjuku), 50.0)
        assertEquals(0.0, Geo.distanceMeters(tokyo, tokyo), 1e-9)
    }

    @Test
    fun bearingCardinalDirections() {
        val origin = LatLng(0.0, 0.0)
        assertEquals(0.0, Geo.bearingDegrees(origin, LatLng(1.0, 0.0)), 1e-6)
        assertEquals(90.0, Geo.bearingDegrees(origin, LatLng(0.0, 1.0)), 1e-6)
        assertEquals(180.0, Geo.bearingDegrees(origin, LatLng(-1.0, 0.0)), 1e-6)
        assertEquals(270.0, Geo.bearingDegrees(origin, LatLng(0.0, -1.0)), 1e-6)
    }

    @Test
    fun interpolateMidpointIsHalfway() {
        val mid = Geo.interpolate(tokyo, shinjuku, 0.5)
        val total = Geo.distanceMeters(tokyo, shinjuku)
        assertEquals(total / 2, Geo.distanceMeters(tokyo, mid), 0.5)
        assertEquals(total / 2, Geo.distanceMeters(mid, shinjuku), 0.5)
        assertEquals(tokyo, Geo.interpolate(tokyo, shinjuku, 0.0).let { snap(it, tokyo) })
    }

    @Test
    fun routeWithWaypoints() {
        val a = LatLng(0.0, 0.0)
        val b = LatLng(0.0, 0.01)
        val c = LatLng(0.01, 0.01)
        val route = Route(listOf(a, b, b, c)) // 長さ 0 の区間を含む
        val ab = Geo.distanceMeters(a, b)
        assertEquals(ab + Geo.distanceMeters(b, c), route.totalMeters, 1e-6)

        val onFirst = route.positionAt(ab / 2)
        assertEquals(90.0, onFirst.bearing, 1e-6)
        val atCorner = route.positionAt(ab)
        assertEquals(0.0, Geo.distanceMeters(atCorner.point, b), 0.01)
        val onLast = route.positionAt(ab + 10)
        assertEquals(0.0, onLast.bearing, 1e-3)
        assertEquals(0.0, Geo.distanceMeters(route.positionAt(1e9).point, c), 0.01)
        assertThrows(IllegalArgumentException::class.java) { Route(listOf(a)) }
    }

    @Test
    fun walkSimulatorMovesAtConstantSpeed() {
        val route = Route(listOf(tokyo, shinjuku))
        val sim = WalkSimulator(route, 1.4)
        val p = sim.advance(100.0)
        assertEquals(140.0, sim.traveledMeters, 1e-9)
        assertEquals(140.0, Geo.distanceMeters(tokyo, p.point), 0.5)
        assertEquals(route.totalMeters - 140.0, sim.remainingMeters, 1e-6)
        assertEquals(sim.remainingMeters / 1.4, sim.remainingSeconds, 1e-6)

        // 1 秒あたりの移動量が速度を超えない（瞬間移動しない）
        var prev = p.point
        repeat(60) {
            val next = sim.advance(1.0).point
            assertTrue(Geo.distanceMeters(prev, next) <= 1.4 + 0.01)
            prev = next
        }
    }

    @Test
    fun walkSimulatorPauseAndArrive() {
        val sim = WalkSimulator(Route(listOf(tokyo, shinjuku)), 2.0)
        sim.advance(10.0)
        sim.paused = true
        sim.advance(1000.0)
        assertEquals(20.0, sim.traveledMeters, 1e-9)
        assertEquals(0.0, sim.currentSpeed, 0.0)
        sim.paused = false
        val end = sim.advance(1e6)
        assertTrue(sim.arrived)
        assertEquals(0.0, sim.currentSpeed, 0.0)
        assertEquals(0.0, Geo.distanceMeters(end.point, shinjuku), 0.01)
        assertFalse(sim.paused)
    }

    @Test
    fun walkSimulatorValidatesSpeed() {
        val route = Route(listOf(tokyo, shinjuku))
        assertThrows(IllegalArgumentException::class.java) { WalkSimulator(route, 0.0) }
        assertThrows(IllegalArgumentException::class.java) { WalkSimulator(route, 51.0) }
        assertThrows(IllegalArgumentException::class.java) { WalkSimulator(route, Double.POSITIVE_INFINITY) }
        assertThrows(IllegalArgumentException::class.java) { WalkSimulator(route, 1.4).advance(-1.0) }
    }

    @Test
    fun formatsStatusForCli() {
        val text = MockLocationService.formatStatus(
            MockLocationService.Status(
                state = MockLocationService.State.WALKING,
                position = tokyo,
                speedMps = 1.4,
                remainingMeters = 100.0,
                remainingSeconds = 71.4,
                providers = listOf("gps", "network"),
            ),
        )
        assertTrue(text.contains("state=WALKING"))
        assertTrue(text.contains("position=35.6812360,139.7671250"))
        assertTrue(text.contains("providers=gps,network"))
        assertEquals("0:01:11", MockLocationService.formatDuration(71.4))
    }

    private fun snap(p: LatLng, expected: LatLng): LatLng =
        if (Geo.distanceMeters(p, expected) < 0.001) expected else p
}
