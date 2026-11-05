package app.lentil

import java.time.ZoneId
import java.util.TimeZone
import org.junit.Assert.assertEquals
import org.junit.Test

class DeviceClockTest {
    @Test
    fun `the clock follows a change of the device's time zone`() {
        val before = TimeZone.getDefault()
        try {
            TimeZone.setDefault(TimeZone.getTimeZone("Europe/Lisbon"))
            assertEquals(ZoneId.of("Europe/Lisbon"), DeviceClock.zone)
            TimeZone.setDefault(TimeZone.getTimeZone("Pacific/Auckland"))
            assertEquals(ZoneId.of("Pacific/Auckland"), DeviceClock.zone)
        } finally {
            TimeZone.setDefault(before)
        }
    }
}
