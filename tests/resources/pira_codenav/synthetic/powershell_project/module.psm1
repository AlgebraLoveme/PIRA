enum Mode {
    Fast
    Safe
}

class Widget {
    [string] $Name

    Widget([string] $name) {
        $this.Name = $name
    }

    [string] Label() {
        return $this.Name
    }
}

function Get-Widget {
    param([string] $Name)
    return [Widget]::new($Name)
}
