package example

enum class State {
    Ready,
    Stopped,
}

interface Labelled {
    fun label(): String
}

data class Model(private val name: String) : Labelled {
    val normalized = name.lowercase()

    override fun label(): String = normalized
}

typealias ModelFactory = (String) -> Model
