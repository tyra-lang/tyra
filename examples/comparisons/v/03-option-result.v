// 03-option-result.v
// Option and Result usage patterns.
// V uses optionals (?T) and result types (!T).

fn find_user(id int) ?string {
	match id {
		1 { return 'alice' }
		2 { return 'bob' }
		else { return none }
	}
}

// V uses `or` blocks for handling optionals
fn user_greeting(id int) ?string {
	name := find_user(id) or { return none }
	return 'hello, ${name}'
}

// V's error handling uses !T for results with errors
fn get_user_result(id int) !string {
	if id <= 0 {
		return error('invalid id')
	}
	name := find_user(id) or { return error('not found') }
	return name
}

fn main() {
	if greeting := user_greeting(1) {
		println(greeting)
	} else {
		println('not found')
	}

	if name := get_user_result(0) {
		println('found: ${name}')
	} else {
		println(err.msg())
	}
}
