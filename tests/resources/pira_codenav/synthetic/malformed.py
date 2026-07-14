class Incomplete:
    def still_visible(self, value: int) -> int:
        if value:
            return value
        return 0


broken = (
