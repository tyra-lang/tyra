// 08-async-tasks.v
// Concurrent task spawning.
// V has coroutines (spawn) and channels for concurrency.

import net.http

fn fetch(url string) !string {
	resp := http.get(url) or { return error('network error: ${err.msg()}') }
	return resp.body
}

fn fetch_all(urls []string) ![]string {
	// V uses spawn for coroutines and shared channels for results
	ch := chan !string{cap: urls.len}

	for url in urls {
		spawn fn [url, ch] () {
			result := fetch(url)
			ch <- result
		}()
	}

	mut results := []string{}
	for _ in 0 .. urls.len {
		result := <-ch or { return error('fetch failed') }
		body := result or { return error('fetch failed: ${err.msg()}') }
		results << body
	}
	return results
}

fn main() {
	urls := [
		'https://api.example.com/a',
		'https://api.example.com/b',
		'https://api.example.com/c',
	]

	results := fetch_all(urls) or {
		println('error: ${err.msg()}')
		return
	}
	for item in results {
		println(item)
	}
}
