package com.example.model;

public record User(String name) {
    public static User anonymous() {
        return new User("anonymous");
    }
}
