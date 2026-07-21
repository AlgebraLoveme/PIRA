package com.example;

import java.util.List;
import com.example.model.User;

public final class App {
    private final User user;

    public App(User user) {
        this.user = user;
    }

    public List<String> names() {
        return List.of(user.name());
    }
}
