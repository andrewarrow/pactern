// Conventional Go implementations. Expected results are never read here.
package main

import (
	"encoding/json"
	"fmt"
	"io"
	"math"
	"os"
	"strconv"
)

type request struct {
	ID   string            `json:"id"`
	Task string            `json:"task"`
	Args []json.RawMessage `json:"args"`
}

type outcome struct {
	Kind     string `json:"kind"`
	Value    any    `json:"value,omitempty"`
	Category string `json:"category,omitempty"`
}

type pacts struct {
	Precondition  string `json:"precondition"`
	Postcondition string `json:"postcondition"`
}

type response struct {
	ID      string  `json:"id"`
	Outcome outcome `json:"outcome"`
	Pacts   *pacts  `json:"pacts,omitempty"`
}

func decode[T any](raw json.RawMessage) T {
	var value T
	if err := json.Unmarshal(raw, &value); err != nil {
		panic(err)
	}
	return value
}

func integer(raw json.RawMessage) int64 {
	value, err := strconv.ParseInt(decode[string](raw), 10, 64)
	if err != nil {
		panic(err)
	}
	return value
}

func add(left, right int64) (int64, bool) {
	if (right > 0 && left > math.MaxInt64-right) || (right < 0 && left < math.MinInt64-right) {
		return 0, false
	}
	return left + right, true
}

func trap(category string) outcome { return outcome{Kind: "trap", Category: category} }
func returned(value any) outcome   { return outcome{Kind: "return", Value: value} }
func decimal(value int64) string   { return strconv.FormatInt(value, 10) }

type reserveResult struct {
	Tag   string `json:"tag"`
	Value string `json:"value,omitempty"`
	Error string `json:"error,omitempty"`
}

func reserve(available, amount int64) reserveResult {
	if amount <= 0 {
		return reserveResult{Tag: "Err", Error: "InvalidAmount"}
	}
	if amount > available {
		return reserveResult{Tag: "Err", Error: "InsufficientQuota"}
	}
	return reserveResult{Tag: "Ok", Value: decimal(available - amount)}
}

func reserveSafety(available, amount int64, result reserveResult) bool {
	if result.Tag == "Ok" {
		remaining, err := strconv.ParseInt(result.Value, 10, 64)
		return err == nil && amount > 0 && amount <= available && remaining == available-amount && remaining >= 0
	}
	if result.Tag != "Err" {
		return false
	}
	if result.Error == "InvalidAmount" {
		return amount <= 0
	}
	if result.Error == "InsufficientQuota" {
		return amount > 0 && amount > available
	}
	return false
}

func execute(req request) response {
	result := response{ID: req.ID}
	switch req.Task {
	case "quota":
		if len(req.Args) != 2 {
			panic("quota arity")
		}
		available, amount := integer(req.Args[0]), integer(req.Args[1])
		if available < 0 {
			result.Outcome = trap("precondition")
			result.Pacts = &pacts{"fail", "not_reached"}
			break
		}
		value := reserve(available, amount)
		if reserveSafety(available, amount, value) {
			result.Pacts = &pacts{"pass", "pass"}
			result.Outcome = returned(value)
		} else {
			result.Pacts = &pacts{"pass", "fail"}
			result.Outcome = trap("postcondition")
		}
	case "pair_totals":
		if len(req.Args) != 1 {
			panic("pair_totals arity")
		}
		values := decode[[]json.RawMessage](req.Args[0])
		var totals []string
		for _, start := range []int{0, 2} {
			if start+1 >= len(values) {
				result.Outcome = trap("bounds")
				break
			}
			total, ok := add(integer(values[start]), integer(values[start+1]))
			if !ok {
				result.Outcome = trap("integer_overflow")
				break
			}
			totals = append(totals, decimal(total))
		}
		if result.Outcome.Kind == "" {
			result.Outcome = returned(totals)
		}
	case "summarize":
		if len(req.Args) != 3 {
			panic("summarize arity")
		}
		label := decode[string](req.Args[0])
		total, ok := add(integer(req.Args[1]), integer(req.Args[2]))
		if !ok {
			result.Outcome = trap("integer_overflow")
		} else {
			result.Outcome = returned(map[string]any{"label": label, "total": decimal(total), "active": total > 0})
		}
	default:
		panic("unknown task: " + req.Task)
	}
	return result
}

func main() {
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		panic(err)
	}
	var requests []request
	if err := json.Unmarshal(input, &requests); err != nil {
		panic(err)
	}
	results := make([]response, 0, len(requests))
	for _, req := range requests {
		results = append(results, execute(req))
	}
	output, err := json.Marshal(results)
	if err != nil {
		panic(err)
	}
	fmt.Println(string(output))
}
