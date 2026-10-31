package app.lentil

import android.app.Application

class LentilApp : Application() {
    lateinit var container: AppContainer
        private set

    override fun onCreate() {
        super.onCreate()
        container = AppContainer(this)
        // Created once on first start (spec section 9).
        container.deviceId()
    }
}
