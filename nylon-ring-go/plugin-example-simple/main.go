package main

import (
	"time"

	"github.com/AssetsArt/nylon-ring/nylon-ring-go/sdk"
)

func main() {
	// Create plugin
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
	plugin.Handle("unary", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// SDK automatically calls this in a goroutine, so you can do blocking work
		// Simulate work (DB call, network, etc.)
		time.Sleep(2 * time.Second)

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

		// End stream
		callback(sdk.Response{
			Status: sdk.StatusStreamEnd,
			Data:   nil,
		})
	})

	// Build and register plugin
	sdk.BuildPlugin(plugin)
}
