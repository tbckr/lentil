package app.lentil

import java.time.Clock
import java.time.Instant
import java.time.ZoneId

/**
 * The system clock in the device's current time zone. [Clock.systemDefaultZone] keeps the zone it
 * was created in, so after the phone changes time zones on a trip, "today" would stay the old
 * zone's date until the app restarts. Android resets the default zone when the device's changes,
 * and this clock reads it on every call.
 */
internal object DeviceClock : Clock() {
    override fun getZone(): ZoneId = ZoneId.systemDefault()

    override fun withZone(zone: ZoneId): Clock = Clock.system(zone)

    override fun instant(): Instant = Instant.now()
}
