package main

import (
	"fmt"

	"github.com/nylon-ring/nylon-ring-go/sdk"
)

func init() {
	plugin := sdk.NewPlugin("nylon-ring-bench-plugin", "0.1.0")

	plugin.HandleSync("unary", func(req sdk.Request, _payload []byte, callback func(sdk.Response)) {
		callback(sdk.Response{
			Status: sdk.StatusOk,
			Data:   []byte("OK: " + req.Path),
		})
	})

	plugin.HandleSync("stream", func(req sdk.Request, payload []byte, callback func(sdk.Response)) {
		// Send 10 frames
		for i := 0; i < 10; i++ {
			callback(sdk.Response{
				Status: sdk.StatusOk,
				Data:   []byte("frame " + fmt.Sprint(i)),
			})
		}
		// End stream
		callback(sdk.Response{
			Status: sdk.StatusStreamEnd,
			Data:   nil,
		})
	})

	sdk.RegisterPlugin(plugin)
}

func main() {}
