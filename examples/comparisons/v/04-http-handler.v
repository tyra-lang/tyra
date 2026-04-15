// 04-http-handler.v
// HTTP server with multiple route handlers.
// V has vweb built into the standard library.

import vweb

struct App {
	vweb.Context
}

['/health']
pub fn (mut app App) health() vweb.Result {
	return app.text('ok')
}

['/greet']
pub fn (mut app App) greet() vweb.Result {
	name := app.query['name'] or { return app.text('missing name') }
	return app.text('hello, ${name}')
}

fn main() {
	vweb.run(&App{}, 8080)
}
