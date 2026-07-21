<?php

namespace App;

trait Named
{
    public function name(): string
    {
        return $this->name;
    }
}

interface Labelled
{
    public function label(): string;
}

enum State
{
    case Ready;
    case Stopped;
}

class Model implements Labelled
{
    use Named;

    private string $name;

    public function __construct(string $name)
    {
        $this->name = $name;
    }

    public function label(): string
    {
        return $this->name();
    }
}

function normalize(string $value): string
{
    return strtolower($value);
}
