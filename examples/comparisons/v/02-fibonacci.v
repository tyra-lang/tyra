// 02-fibonacci.v
// Recursive Fibonacci.

fn fib(n int) int {
	match n {
		0 { return 0 }
		1 { return 1 }
		else { return fib(n - 1) + fib(n - 2) }
	}
}

fn main() {
	result := fib(10)
	println('fib(10) = ${result}')
}
