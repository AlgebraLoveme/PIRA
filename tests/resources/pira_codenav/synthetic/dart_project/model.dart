abstract class Labelled {
  String get label;
}

enum State { ready, failed }

class User implements Labelled {
  final String name;

  User(this.name);

  @override
  String get label => name;

  String render() => label;
}

typedef Mapper = String Function(String);

String normalize(String value) => value.trim();
