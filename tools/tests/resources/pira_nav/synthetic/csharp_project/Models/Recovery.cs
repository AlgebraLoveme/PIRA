namespace Pira.Models;

public ref struct RefBox<T>
{
    private ref T _value;

    public RefBox(ref T value)
    {
        _value = ref value;
    }

    public static T Identity(T value)
#if NET
        where T : allows ref struct
#endif
        => value;

    public static unsafe T Read(void* pointer)
    {
        return *(T*)pointer;
    }
}

public static class RefBoxExtensions
{
    extension<T>(RefBox<T> value)
    {
        public bool IsValid => true;

        public static RefBox<T> operator +(RefBox<T> left, RefBox<T> right) => left;
    }
}
