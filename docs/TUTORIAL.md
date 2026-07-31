# Build a Weather CLI with Nulang

> A step-by-step guide that builds a real command-line weather tool.
> Each step introduces one language concept and produces runnable code.
> By the end you'll have a working program that fetches live weather data,
> parses JSON, handles errors, and writes reports to disk.

## What We're Building

A CLI tool that takes a city name, fetches current weather from the free
[Open-Meteo API](https://open-meteo.com/), and displays temperature, wind
speed, and conditions. Along the way you'll learn variables, functions, HTTP,
JSON parsing, pattern matching, records, environment variables, file I/O,
and error handling — the real building blocks of any Nulang program.

**Prerequisites:** Nulang installed (`nulang --version`). Network access.

---

## Step 1: Project Setup

Create a new Nulang project (or work with a single file — we'll use a
standalone script to keep things simple):

```bash
mkdir weather-cli && cd weather-cli
```

Create `weather.nula` — your entry point. We'll build it up step by step.

> **Package manager alternative:** `nulang nula new weather-cli` scaffolds a
> full package with `Nulang.toml` and `src/main.nula`. The code in this
> tutorial works identically in either layout.

Verify everything works:

```bash
nulang --eval 'perform IO.print("Ready!")'
```

Expected output:

```
Ready!
```

---

## Step 2: Hello, Weather

Open `weather.nula` and write your first line:

```nula
perform IO.print("Hello, Weather CLI!")
```

Run it:

```bash
nulang weather.nula
```

Expected output:

```
Hello, Weather CLI!
```

**What's happening:** `IO.print` is a *built-in effect*. The `perform`
keyword tells Nulang "execute this effect now." Built-in effects (`IO`,
`Http`, `FS`, `System`, `Env`) are wired into the VM — no import needed.
Every effect call requires `perform`.

---

## Step 3: Variables and String Concatenation

Nulang uses `let` for immutable bindings. Use `+` to join strings:

```nula
let city = "Berlin"
let message = "Weather for " + city + ":"
perform IO.print(message)
```

Run it:

```bash
nulang weather.nula
```

Expected output:

```
Weather for Berlin:
```

**Key points:**
- `let` bindings can't be reassigned. Use `var` if you need mutation.
- `+` concatenates strings; use `perform Int.to_string(n)` or
  `perform Float.to_string(n)` to convert numbers first.
- No semicolons needed. Blocks use `{ }` for grouping.

> Replace `weather.nula` with just this code — we'll rebuild it at each step.

---

## Step 4: Functions

Functions are values. Define them with `fn(params) { body }`:

```nula
let greet = fn(city: String) {
    perform IO.print("Weather for " + city + ":")
}

greet("Berlin")
greet("Tokyo")
```

Run it:

```
Weather for Berlin:
Weather for Tokyo:
```

You can also write a top-level function with `fn name(params) { ... }`:

```nula
fn greet(city: String) {
    perform IO.print("Weather for " + city + ":")
}

greet("Berlin")
```

Both forms are equivalent. The `fn(name) { body }` closure form is handy for
passing functions as arguments; the `fn name(params) { body }` form is
idiomatic for top-level definitions.

**Functions return the last expression** in their body. Explicit `return` is
not needed.

---

## Step 5: Reading Command-Line Arguments

Let's read a city name from the command line. `System.arg(n)` returns the
n-th argument, or `nil` if none was provided:

```nula
let city = perform System.arg(2)

perform IO.print("City: " + city)
```

Run with and without an argument:

```bash
nulang weather.nula Berlin
# → City: Berlin

nulang weather.nula
# → City: nil
```

**Argument indexing:** `System.arg(0)` is the program name, `System.arg(1)`
is the script path, and `System.arg(2)` is the first user argument.

When no argument is given, `city` is `nil` — Nulang's sentinel for "no
value." We'll handle that next.

---

## Step 6: If / Else — Check for Required Arguments

Use `if ... then ... else` to handle missing input gracefully:

```nula
let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    perform IO.print("Fetching weather for " + city + "...")
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
    perform IO.print("")
    perform IO.print("Example:")
    perform IO.print("  nulang weather.nula Berlin")
}
```

Run it:

```bash
nulang weather.nula
# → Usage: nulang weather.nula <city>
# →
# → Example:
# →   nulang weather.nula Berlin

nulang weather.nula Berlin
# → Fetching weather for Berlin...
```

**Key points:**
- Conditions don't need parentheses: `has_city` not `(has_city)`.
- `then` separates the condition from the true branch.
- `!= nil` checks if a value exists; `== nil` checks if it doesn't.

---

## Step 7: Environment Variables

This tutorial uses the free Open-Meteo API (no key required). But for
production apps you might want a paid service with an API key. Let's
teach `Env.get` with an optional key:

```nula
let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    // Check for an optional API key
    let api_key = perform Env.get("WEATHER_API_KEY")
    let has_key = api_key != nil

    if has_key then {
        perform IO.print("Using API key: " + api_key)
    } else {
        perform IO.print("No API key set — using free tier")
    }

    perform IO.print("Fetching weather for " + city + "...")
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Try it:

```bash
nulang weather.nula Berlin
# → No API key set — using free tier
# → Fetching weather for Berlin...

WEATHER_API_KEY=abc123 nulang weather.nula Berlin
# → Using API key: abc123
# → Fetching weather for Berlin...
```

**Key point:** `Env.get` returns `nil` when the variable isn't set. Use
`!= nil` to check and provide a fallback.

---

## Step 8: Building the API URL

We'll use the Open-Meteo API, which takes latitude and longitude. Rather
than require the user to type coordinates, let's build a small city lookup
table:

```nula
let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    // City → coordinates lookup
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"   // default to Berlin

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    // Build the API URL
    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    perform IO.print("URL: " + url)
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Run it:

```bash
nulang weather.nula Tokyo
# → URL: https://api.open-meteo.com/v1/forecast?latitude=35.68&longitude=139.76&current_weather=true
```

**What we did:** built a URL with query parameters using string concatenation.
`+` chains work across multiple lines — Nulang reads them as one expression.

> In the bonus section we'll add real geocoding via an API call, eliminating
> the hardcoded lookup table.

---

## Step 9: Making HTTP Requests

Now let's actually fetch data. `Http.get(url)` returns the response body
as a string — or `nil` if the request fails:

```nula
let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    perform IO.print("Fetching weather for " + city + "...")

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let size = perform String.length(response)
        perform IO.print("Received " + perform Int.to_string(size) + " bytes")
        perform IO.print(response)
    } else {
        perform IO.print("Error: could not reach the weather API.")
        perform IO.print("Check your network connection and try again.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Run it — you should see raw JSON:

```bash
nulang weather.nula Berlin
# → Fetching weather for Berlin...
# → Received 235 bytes
# → {"latitude":52.52,"longitude":13.41,"current_weather":{"temperature":...
```

**Key points:**
- `perform Http.get(url)` hits the network — it's an effect.
- `response != nil` checks if the request succeeded.
- `perform String.length(s)` returns the byte count of a string.
- `perform Int.to_string(n)` converts an integer for printing.

---

## Step 10: Parsing JSON

Import `stdlib::json` to get `parse`, `stringify`, `get_string`, and
`get_number`. The `parse` function turns a JSON string into a `JsonValue`:

```nula
import stdlib::json

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    perform IO.print("Fetching weather for " + city + "...")

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let parsed = parse(response)
        let pretty = stringify(parsed)
        perform IO.print("Parsed JSON: " + pretty)
    } else {
        perform IO.print("Error: could not reach the weather API.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

**Module imports** use `::` (not `.`): `import stdlib::json`. Imported names
are *unqualified* — call `parse(...)` and `stringify(...)` directly.

> **Important:** Run from the project root so the stdlib path is resolvable.
> In development, set `NULANG_STDLIB` to point at `src/stdlib/`. The
> examples use `nulang weather.nula` from the project root.

---

## Step 11: Pattern Matching on JSON

`JsonValue` is a variant type with six constructors. Use `match` to
inspect which kind of value you have and extract data:

```nula
import stdlib::json

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let parsed = parse(response)

        // Classify and extract based on JSON shape
        let summary = match parsed {
            JsonNull       => "Response was null — unexpected!",
            JsonBool(b)    => if b then "true" else "false",
            JsonNumber(n)  => "number: " + perform Float.to_string(n),
            JsonString(s)  => "string (" +
                              perform Int.to_string(perform String.length(s)) +
                              " chars)",
            JsonArray(a)   => "array with " +
                              perform Int.to_string(perform Array.length(a)) +
                              " items",
            JsonObject(_)  => "weather object"
        }

        perform IO.print("Result: " + summary)
    } else {
        perform IO.print("Error: could not reach the weather API.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Run it:

```bash
nulang weather.nula Berlin
# → Result: weather object
```

**How `match` works:** Each arm `Pattern => expression` tests the value.
Patterns bind variables — `JsonObject(_)` matches any object, while
`JsonString(s)` binds the string payload to `s`. The underscore `_`
discards a binding you don't need.

> Nulang also supports `match value with { | pattern => ... }` syntax
> with leading `|` and guards (`if`). We use the compact form here.

---

## Step 12: Records for Structured Data

Records group related data with named fields. Let's define a `Weather`
record and populate it:

```nula
import stdlib::json

// A Weather record to hold structured data
let make_weather = fn(city: String, temp: Float, wind: Float, code: Int) {
    { city: city, temp: temp, wind: wind, weather_code: code }
}

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let parsed = parse(response)

        // For now, use hardcoded values; we'll extract from JSON next
        let weather = make_weather(city, 21.5, 12.3, 2)

        perform IO.print("City:  " + weather.city)
        perform IO.print("Temp:  " + perform Float.to_string(weather.temp) + "°C")
        perform IO.print("Wind:  " + perform Float.to_string(weather.wind) + " km/h")
        perform IO.print("Code:  " + perform Int.to_string(weather.weather_code))
    } else {
        perform IO.print("Error: could not reach the weather API.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

**Record syntax:**
- Creation: `{ field: value, ... }` — using colons.
- Field access: `record.field` — using dots.
- Update: `{ base .. field = new_val }` — using `..` and `=`.

---

## Step 13: String Formatting and Display

Now let's format the weather data into a readable report. We'll convert
the numeric weather code into a human-readable description:

```nula
import stdlib::json

let make_weather = fn(city: String, temp: Float, wind: Float, code: Int) {
    { city: city, temp: temp, wind: wind, weather_code: code }
}

// Map WMO weather codes to descriptions
let describe_code = fn(code: Int) {
    if code == 0        then "Clear sky"
    else if code <= 3   then "Partly cloudy"
    else if code <= 48  then "Fog"
    else if code <= 55  then "Drizzle"
    else if code <= 65  then "Rain"
    else if code <= 75  then "Snow"
    else if code <= 82  then "Rain showers"
    else if code <= 99  then "Thunderstorm"
    else "Unknown"
}

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let parsed = parse(response)

        // Placeholder values — we'll extract real ones in Step 16
        let weather = make_weather(city, 21.5, 12.3, 2)

        // Build a formatted report
        let report =
            "══════════════════════════\n" +
            " Weather for " + weather.city + "\n" +
            "══════════════════════════\n" +
            " Temperature:  " + perform Float.to_string(weather.temp) + " °C\n" +
            " Wind speed:   " + perform Float.to_string(weather.wind) + " km/h\n" +
            " Conditions:   " + describe_code(weather.weather_code) + "\n" +
            "══════════════════════════"

        perform IO.print(report)
    } else {
        perform IO.print("Error: could not reach the weather API.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Run it:

```
══════════════════════════
 Weather for Berlin
══════════════════════════
 Temperature:  21.5 °C
 Wind speed:   12.3 km/h
 Conditions:   Partly cloudy
══════════════════════════
```

**Conversion functions:** `Float.to_string` and `Int.to_string` are
built-in effects — always call them with `perform`. They convert numbers
to strings for display or concatenation.

---

## Step 14: Error Handling — What If the API Fails?

The weather API can return errors (bad coordinates, rate limiting, etc.).
Let's check for error indicators in the response:

```nula
import stdlib::json

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    let response = perform Http.get(url)

    // Check: did the HTTP request itself fail?
    let got_response = response != nil

    if got_response then {
        let parsed = parse(response)

        // Check: did the API return an error field?
        let error_msg = get_string(parsed, "error", "")
        let has_error = error_msg != ""

        if has_error then {
            perform IO.print("API error: " + error_msg)
        } else {
            // Check: does the response have current_weather?
            let cw_str = get_string(parsed, "current_weather", "")
            let has_weather = cw_str != ""

            if has_weather then {
                perform IO.print("Weather data received successfully!")
            } else {
                perform IO.print("No current weather in response.")
                perform IO.print("Raw: " + stringify(parsed))
            }
        }
    } else {
        perform IO.print("Network error: could not reach the API.")
        perform IO.print("Check your internet connection.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

**Layered error handling:**
1. `response != nil` — did the HTTP call succeed?
2. `get_string(parsed, "error", "")` — did the API return an error?
3. `get_string(parsed, "current_weather", "")` — is the expected data present?

`get_string(json, key, default)` returns the string value at `key`, or the
default if the field doesn't exist or isn't a string.

---

## Step 15: Saving Results to a File

Use `FS.write` to persist output and `stdlib::datetime` to timestamp it:

```nula
import stdlib::json
import stdlib::datetime

let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    let lat = if city == "Berlin"  then "52.52"
         else if city == "Tokyo"   then "35.68"
         else if city == "New York" then "40.71"
         else if city == "London"  then "51.51"
         else "52.52"

    let lon = if city == "Berlin"  then "13.41"
         else if city == "Tokyo"   then "139.76"
         else if city == "New York" then "-74.01"
         else if city == "London"  then "-0.13"
         else "13.41"

    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + lat +
              "&longitude=" + lon +
              "&current_weather=true"

    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        // Capture the current time
        let now = now()
        let ts = perform Int.to_string(now.year) + "-" +
                 perform Int.to_string(now.month) + "-" +
                 perform Int.to_string(now.day) + " " +
                 perform Int.to_string(now.hour) + ":" +
                 perform Int.to_string(now.minute) + ":" +
                 perform Int.to_string(now.second)

        perform IO.print("Fetched at: " + ts)

        // Build a report and write it to disk
        let report = "Weather Report — " + city + "\n" +
                     "Fetched: " + ts + "\n" +
                     "Raw JSON:\n" + response + "\n"

        let out_path = "weather_report.txt"
        perform FS.write(out_path, report)
        perform IO.print("Report saved to " + out_path)
    } else {
        perform IO.print("Error: could not reach the weather API.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
}
```

Run it, then check the file:

```bash
nulang weather.nula Berlin
# → Fetched at: 2024-7-31 14:30:5
# → Report saved to weather_report.txt

cat weather_report.txt
# → Weather Report — Berlin
# → Fetched: 2024-7-31 14:30:5
# → Raw JSON:
# → {"latitude":52.52,...}
```

**`import stdlib::datetime`** gives you `now()` — a `DateTime` record with
`.year`, `.month`, `.day`, `.hour`, `.minute`, `.second` fields, all `Int`.

**`FS.write(path, content)`** writes a string to disk. `FS.read(path)`
reads it back. Both are built-in effects — `perform` required.

---

## Step 16: Putting It All Together

Here's the complete weather CLI. Replace `weather.nula` with this:

```nula
import stdlib::json
import stdlib::datetime

// ── City coordinates lookup ──────────────────────────────────────────────

let get_coords = fn(city: String) {
    if city == "Berlin"     then { lat: "52.52",  lon: "13.41" }
    else if city == "Tokyo"      then { lat: "35.68",  lon: "139.76" }
    else if city == "New York"    then { lat: "40.71",  lon: "-74.01" }
    else if city == "London"     then { lat: "51.51",  lon: "-0.13" }
    else if city == "Paris"      then { lat: "48.85",  lon: "2.35" }
    else if city == "Sydney"     then { lat: "-33.87", lon: "151.21" }
    else if city == "São Paulo"   then { lat: "-23.55", lon: "-46.63" }
    else { lat: "52.52", lon: "13.41" }   // default: Berlin
}

// ── Weather code → description ───────────────────────────────────────────

let describe_code = fn(code: Int) {
    if code == 0        then "Clear sky"
    else if code <= 3   then "Partly cloudy"
    else if code <= 48  then "Fog"
    else if code <= 55  then "Drizzle"
    else if code <= 65  then "Rain"
    else if code <= 75  then "Snow"
    else if code <= 82  then "Rain showers"
    else if code <= 99  then "Thunderstorm"
    else "Unknown"
}

// ── Format timestamp ─────────────────────────────────────────────────────

let format_time = fn(dt) {
    let pad = fn(n: Int) {
        if n < 10 then "0" + perform Int.to_string(n)
        else perform Int.to_string(n)
    }
    perform Int.to_string(dt.year) + "-" +
    pad(dt.month) + "-" +
    pad(dt.day) + " " +
    pad(dt.hour) + ":" +
    pad(dt.minute) + ":" +
    pad(dt.second)
}

// ── Weather record type ──────────────────────────────────────────────────

let make_weather = fn(city: String, temp: Float, wind: Float, code: Int, desc: String) {
    { city: city, temp: temp, wind: wind, weather_code: code, description: desc }
}

// ── Main program ─────────────────────────────────────────────────────────

// 1. Read the city from the command line
let city = perform System.arg(2)
let has_city = city != nil

if has_city then {
    // 2. Look up coordinates
    let coords = get_coords(city)

    // 3. Build the API URL
    let url = "https://api.open-meteo.com/v1/forecast" +
              "?latitude=" + coords.lat +
              "&longitude=" + coords.lon +
              "&current_weather=true"

    // 4. Check for optional API key
    let api_key = perform Env.get("WEATHER_API_KEY")
    let has_key = api_key != nil

    perform IO.print("")
    perform IO.print("═══════════════════════════════════")
    perform IO.print(" Nulang Weather CLI")
    perform IO.print("═══════════════════════════════════")
    perform IO.print("City:    " + city)
    if has_key then {
        perform IO.print("API key: " + api_key)
    } else {
        perform IO.print("API:     Open-Meteo (free tier)")
    }
    perform IO.print("Fetching...")

    // 5. Make the HTTP request
    let response = perform Http.get(url)
    let ok = response != nil

    if ok then {
        let size = perform String.length(response)
        perform IO.print("Received " + perform Int.to_string(size) + " bytes")

        // 6. Parse the JSON
        let parsed = parse(response)

        // 7. Check for API errors
        let error_msg = get_string(parsed, "error", "")
        let has_error = error_msg != ""

        if has_error then {
            perform IO.print("API error: " + error_msg)
        } else {
            // 8. Extract weather data
            //    The current_weather field is a nested JSON object.
            //    get_string on it returns the stringified inner object,
            //    which we re-parse to access temperature, windspeed, etc.
            let cw_str = get_string(parsed, "current_weather", "")
            let has_cw = cw_str != ""

            if has_cw then {
                let cw = parse(cw_str)
                let temp = get_number(cw, "temperature", 0.0)
                let wind = get_number(cw, "windspeed", 0.0)
                let code = get_number(cw, "weathercode", 0.0)
                let code_int = perform Float.to_int(code)
                let desc = describe_code(code_int)

                let weather = make_weather(city, temp, wind, code_int, desc)

                // 9. Display the report
                let report =
                    "────────────────────────────────\n" +
                    " Weather for " + weather.city + "\n" +
                    "────────────────────────────────\n" +
                    " Temperature:  " + perform Float.to_string(weather.temp) + " °C\n" +
                    " Wind speed:   " + perform Float.to_string(weather.wind) + " km/h\n" +
                    " Conditions:   " + weather.description + "\n" +
                    "────────────────────────────────"

                perform IO.print(report)

                // 10. Save to file
                let now = now()
                let ts = format_time(now)

                let file_report =
                    "Weather Report\n" +
                    "==============\n" +
                    "City:         " + weather.city + "\n" +
                    "Fetched at:   " + ts + "\n" +
                    "Temperature:  " + perform Float.to_string(weather.temp) + " °C\n" +
                    "Wind speed:   " + perform Float.to_string(weather.wind) + " km/h\n" +
                    "Conditions:   " + weather.description + "\n" +
                    "\n" +
                    "Raw response:\n" +
                    stringify(parsed) + "\n"

                let out_path = "weather_report.txt"
                perform FS.write(out_path, file_report)
                perform IO.print("")
                perform IO.print("Report saved to " + out_path)
            } else {
                perform IO.print("No current_weather field in response.")
                perform IO.print("Response: " + stringify(parsed))
            }
        }
    } else {
        perform IO.print("Error: could not reach the weather API.")
        perform IO.print("Check your internet connection.")
    }
} else {
    perform IO.print("Usage: nulang weather.nula <city>")
    perform IO.print("")
    perform IO.print("Supported cities: Berlin, Tokyo, New York, London, Paris, Sydney, São Paulo")
    perform IO.print("")
    perform IO.print("Example:")
    perform IO.print("  nulang weather.nula Tokyo")
}
```

Run it:

```bash
nulang weather.nula Tokyo
```

Expected output:

```
═══════════════════════════════════
 Nulang Weather CLI
═══════════════════════════════════
City:    Tokyo
API:     Open-Meteo (free tier)
Fetching...
Received 225 bytes
────────────────────────────────
 Weather for Tokyo
────────────────────────────────
 Temperature:  21.3 °C
 Wind speed:   15.8 km/h
 Conditions:   Partly cloudy
────────────────────────────────

Report saved to weather_report.txt
```

**Congratulations!** You've built a real Nulang application. Here's what
you used:

| Concept | Where |
|---------|-------|
| Effects (`perform`) | `IO.print`, `Http.get`, `FS.write`, `System.arg`, `Env.get` |
| Immutable bindings (`let`) | Variables and functions throughout |
| Functions as values | `make_weather`, `describe_code`, `get_coords`, closures |
| Records | `{ lat: ..., lon: ... }`, `Weather` record |
| If / then / else | Conditionals and city lookup |
| `!= nil` / `== nil` | Nil checks for error handling |
| String concatenation (`+`) | URL building, report formatting |
| HTTP requests | `Http.get(url)` |
| JSON parsing | `parse()`, `stringify()`, `get_string()`, `get_number()` |
| Pattern matching | *(used implicitly via `get_string`/`get_number`)* |
| Numeric conversion | `Int.to_string`, `Float.to_string`, `Float.to_int` |
| Date & time | `now()`, `DateTime` record fields |
| File I/O | `FS.write(path, content)` |
| Module imports | `import stdlib::json`, `import stdlib::datetime` |

---

## Step 17: Running with Different Cities

Try each supported city:

```bash
nulang weather.nula "New York"
nulang weather.nula London
nulang weather.nula Paris
nulang weather.nula Sydney
nulang weather.nula "São Paulo"
```

Try an unsupported city — it defaults to Berlin:

```bash
nulang weather.nula Mumbai
```

Observe that:
- Different cities return different temperatures and wind speeds.
- The report file (`weather_report.txt`) is overwritten each run.
- The timestamp updates on every invocation.

---

## Step 18: Bonus — Formatting Options

Let's add `--json` and `--verbose` flags. Update the end of your program
(after the coordinate lookup and before the API call) to read a second
argument:

```nula
// Check for optional flags after the city name
let flag = perform System.arg(3)
let is_json = flag == "--json"
let is_verbose = flag == "--verbose"
```

Then wrap your output logic:

```nula
if is_json then {
    // --json: print raw parsed JSON
    perform IO.print(stringify(parsed))
} else {
    // Default: human-readable report (the existing code)
    ...
}

if is_verbose then {
    // --verbose: print extra details
    perform IO.print("URL:       " + url)
    perform IO.print("Response:  " + perform Int.to_string(size) + " bytes")
    perform IO.print("Lat/Lon:   " + coords.lat + ", " + coords.lon)
}
```

Run with flags:

```bash
nulang weather.nula Berlin --json
nulang weather.nula Tokyo --verbose
nulang weather.nula London --json --verbose
```

This demonstrates combining positional and flag-style arguments. Add the
flag logic right after reading the city name and before the `if has_city`
block for clean integration.

---

## Troubleshooting

### 1. "I forgot `perform` and got a type error"

```nula
// ❌ WRONG — missing perform
IO.print("hello")
Http.get(url)
Int.to_string(n)

// ✅ CORRECT — every built-in effect needs perform
perform IO.print("hello")
perform Http.get(url)
perform Int.to_string(n)
```

**Why:** Built-in effects (`IO`, `Http`, `FS`, `System`, `Env`, `Int`,
`String`, `Float`, `Array`) are VM-wired and require the `perform` keyword.
Without it, the compiler treats them as unknown functions.

### 2. "My import doesn't work — I used dots instead of colons"

```nula
// ❌ WRONG
import stdlib.json
import stdlib.datetime

// ✅ CORRECT
import stdlib::json
import stdlib::datetime
```

**Why:** Module paths use `::` as the separator. Imported names are
*unqualified* — after `import stdlib::json`, call `parse(...)` directly
(not `json.parse(...)`).

### 3. "I used `== nil` but meant `!= nil` (or vice versa)"

```nula
let response = perform Http.get(url)

// ❌ WRONG — checks for failure but treats it as success
if response == nil then {
    perform IO.print("Got data!")    // this runs when there's NO data!
}

// ✅ CORRECT
if response != nil then {
    perform IO.print("Got data!")    // this runs when data IS present
}
```

**Why:** `nil` means "no value." `response != nil` means "the request
succeeded and we have data." `response == nil` means "the request failed."
Double-check your logic — it's easy to invert by accident.

### 4. "My program runs but prints nothing — what's wrong?"

Make sure you're using `nulang` (not `nula` directly) and that your file
path is correct:

```bash
nulang weather.nula Berlin     # ✅ correct
nulang nula run                # ✅ if using a package with Nulang.toml
```

If you scaffolded with `nulang nula new`, run `nulang nula run` from the
project directory. Arguments may pass differently through the package
manager — check your `Nulang.toml` configuration.

---

## Where to Go Next

- **`docs/GETTING_STARTED.md`** — deeper dive into effects, actors, pattern matching
- **`docs/PITFALLS.md`** — common mistakes and idiomatic fixes
- **`examples/`** — 17 verified example programs covering every language feature
- **`examples/16_realworld.nula`** — the JSON fetcher that inspired this tutorial
- **`examples/17_actor_fetcher.nula`** — multi-URL fetcher with detailed reporting
- **`nulang --repl`** — interactive exploration with `:help`, `:type`, `:load`

---

*Built with Nulang — an actor-based language with algebraic effects and
capability tracking.*
