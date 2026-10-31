package app.lentil.data

import java.io.File
import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.Json

/**
 * Keeps [IntentState] as one JSON file. Writes go to a temporary file first and replace the old
 * file atomically, so a crash never leaves half a file and never loses an intent.
 */
class JsonIntentStore(
    private val file: File,
    private val io: CoroutineDispatcher = Dispatchers.IO,
) : IntentStore {
    override suspend fun read(): IntentState =
        withContext(io) {
            if (file.exists()) json.decodeFromString<IntentFile>(file.readText()).state
            else IntentState()
        }

    override suspend fun write(state: IntentState) =
        withContext(io) {
            val temporary = File(file.parentFile, "${file.name}.tmp")
            temporary.writeText(json.encodeToString(IntentFile.serializer(), IntentFile(state)))
            check(temporary.renameTo(file)) { "cannot replace ${file.name}" }
        }

    companion object {
        /** `op` like the core's JSON for intents (spec section 10 names). */
        val json = Json {
            classDiscriminator = "op"
            ignoreUnknownKeys = true
        }
    }
}

@kotlinx.serialization.Serializable
private data class IntentFile(val version: Int = 1, val state: IntentState) {
    constructor(state: IntentState) : this(1, state)
}
