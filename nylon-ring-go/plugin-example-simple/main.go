package main

import (
	"time"

	"github.com/AssetsArt/nylon-ring/nylon-ring-go/sdk"
)

func init() {
	// Initialize plugin in package-level initialization
	// This ensures it's set up before the plugin is loaded
	plugin := sdk.NewPlugin("nylon-ring-go-plugin", "1.0.0")

	// Set initialization function (optional)
	plugin.OnInit(func() error {
		// Initialize plugin state, connect to DB, etc.
		return nil
	})

	// Set shutdown function (optional)
	plugin.OnShutdown(func() {
		// Cleanup resources
	})

	// Register unary handler
	// Use HandleSync for fast, non-blocking operations
	plugin.HandleSync("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// Prepare response
		response := "OK: " + req.Method + " " + req.Path

		// Send result back
		callback(sdk.Response{
			Status: sdk.StatusOk,
			Data:   []byte(response),
		})
	})

	// Register streaming handler
	plugin.Handle("stream", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// SDK automatically calls this in a goroutine
		// Send multiple frames
		for i := 1; i <= 5; i++ {
			time.Sleep(1 * time.Second)

			// Send frame
			callback(sdk.Response{
				Status: sdk.StatusOk,
				Data:   []byte("Frame " + string(rune('0'+i)) + "/5 from " + req.Path),
			})
		}

		// End stream (use empty bytes, not nil)
		callback(sdk.Response{
			Status: sdk.StatusStreamEnd,
			Data:   []byte{},
		})
	})

	// Register raw handler
	plugin.HandleRaw("echo", func(payload []byte, callback func(sdk.Response)) {
		// Echo back the payload
		callback(sdk.Response{
			Status: sdk.StatusOk,
			Data:   payload,
		})
	})

	// Build and register plugin
	sdk.BuildPlugin(plugin)
}

func main() {
	// This is a plugin library, not a standalone program
	// The main function is required but won't be called when loaded as a plugin
	// Plugin initialization is done in the package-level init above
}
