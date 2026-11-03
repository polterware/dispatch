package classifier

import (
	"bytes"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

// requestTimeout bounds one HTTP attempt; retries add backoff on top.
const requestTimeout = 10 * time.Second

// retries is how many times a rate-limited or overloaded call is tried again.
const retries = 3

// postJSON sends body with a bearer key and decodes the JSON answer into out,
// retrying 429, 529, and 503 with exponential backoff.
func postJSON(ctx context.Context, client *http.Client, url, key string, body, out any) error {
	b, err := json.Marshal(body)
	if err != nil {
		return err
	}
	if client == nil {
		client = &http.Client{Timeout: requestTimeout}
	}
	backoff := 500 * time.Millisecond
	for attempt := 0; ; attempt++ {
		req, err := http.NewRequestWithContext(ctx, http.MethodPost, url, bytes.NewReader(b))
		if err != nil {
			return err
		}
		req.Header.Set("Authorization", "Bearer "+key)
		req.Header.Set("Content-Type", "application/json")
		resp, err := client.Do(req)
		if err != nil {
			return err
		}
		data, _ := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
		resp.Body.Close()
		switch {
		case resp.StatusCode == http.StatusOK:
			if err := json.Unmarshal(data, out); err != nil {
				return fmt.Errorf("bad response: %w", err)
			}
			return nil
		case (resp.StatusCode == 429 || resp.StatusCode == 529 || resp.StatusCode == 503) && attempt < retries:
			select {
			case <-ctx.Done():
				return ctx.Err()
			case <-time.After(backoff):
			}
			backoff *= 2
		default:
			return fmt.Errorf("HTTP %d: %s", resp.StatusCode, snippet(data))
		}
	}
}

func snippet(b []byte) string {
	const max = 200
	if len(b) > max {
		b = b[:max]
	}
	return string(bytes.TrimSpace(b))
}
