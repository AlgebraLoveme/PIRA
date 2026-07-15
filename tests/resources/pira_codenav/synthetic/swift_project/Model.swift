import Foundation

protocol Labelled {
    var label: String { get }
    func render() -> String
}

enum State {
    case ready
    case failed(String)
}

struct User: Labelled {
    let name: String
    var label: String { name }

    init(name: String) {
        self.name = name
    }

    func render() -> String {
        label
    }
}

extension User {
    static func sample() -> User {
        User(name: "PIRA")
    }
}

func normalize(_ value: String) -> String {
    value.trimmingCharacters(in: .whitespaces)
}
