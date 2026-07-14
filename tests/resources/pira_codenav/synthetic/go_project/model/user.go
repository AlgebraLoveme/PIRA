package model

const (
	DefaultName = "Ada"
	MaxUsers    = 32
)

var DefaultNamePointer = new("Ada")

type Labeler interface {
	Label() string
}

type User struct {
	Name string
}

func NewUser(name string) User {
	return User{Name: name}
}

func (user User) Label() string {
	return user.Name
}
