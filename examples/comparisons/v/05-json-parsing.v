// 05-json-parsing.v
// JSON parsing with custom error types.
// V has built-in json.decode with struct mapping.

import json

struct User {
	name string
}

fn load_user_name(input string) !string {
	user := json.decode(User, input) or {
		return error('parse failed: ${err.msg()}')
	}
	if user.name == '' {
		return error('missing key: name')
	}
	return user.name
}

fn main() {
	name := load_user_name('{"name": "alice"}') or {
		println(err.msg())
		return
	}
	println('user: ${name}')
}
