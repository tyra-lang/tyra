// 10-data-modeling.v
// Data modeling with structs and interfaces.
// V has structs (mutable by default), interfaces, and methods.

struct Point {
	x f64
	y f64
}

fn (p Point) to_string() string {
	return '(${p.x}, ${p.y})'
}

struct UserId {
	id int
}

struct User {
mut:
	id    UserId
	name  string
	email string
}

fn (u User) to_string() string {
	return 'User(${u.id.id}, ${u.name})'
}

fn distance_squared(a Point, b Point) f64 {
	dx := a.x - b.x
	dy := a.y - b.y
	return dx * dx + dy * dy
}

// V: structs are passed by value by default.
// Use `mut` parameter to modify in place.
fn rename(mut user User, new_name string) {
	user.name = new_name
}

fn main() {
	origin := Point{x: 0.0, y: 0.0}
	p := Point{x: 3.0, y: 4.0}
	p2 := Point{
		...p
		x: 1.0
	}

	println(origin.to_string())
	println(p.to_string())
	println(p2.to_string())

	dist_sq := distance_squared(origin, p)
	println('distance squared: ${dist_sq}')

	// V supports == for struct comparison
	if origin == p {
		println('same point')
	} else {
		println('different points')
	}

	mut user := User{
		id: UserId{id: 1}
		name: 'alice'
		email: 'alice@example.com'
	}
	rename(mut user, 'alice smith')
	println(user.to_string())

	id1 := UserId{id: 1}
	id2 := UserId{id: 2}
	if id1.id < id2.id {
		println('id1 is smaller')
	} else {
		println('id2 is smaller or equal')
	}
}
