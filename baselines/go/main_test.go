package main

import (
	"encoding/json"
	"math"
	"testing"
)

func TestCheckedAdd(t *testing.T) {
	for _, pair := range [][2]int64{{math.MaxInt64, 1}, {math.MinInt64, -1}} {
		if _, ok := add(pair[0], pair[1]); ok {
			t.Fatalf("overflow accepted: %v", pair)
		}
	}
	if value, ok := add(math.MinInt64, math.MaxInt64); !ok || value != -1 {
		t.Fatal("valid boundary sum rejected")
	}
}

func TestGuardRejectsFaultyRepairs(t *testing.T) {
	for _, candidate := range []struct {
		amount int64
		result reserveResult
	}{
		{4, reserveResult{Tag: "Ok", Value: "-1"}},
		{0, reserveResult{Tag: "Ok", Value: "3"}},
		{2, reserveResult{Tag: "Err", Error: "InvalidAmount"}},
		{4, reserveResult{Tag: "Err", Error: "InvalidAmount"}},
	} {
		if reserveSafety(3, candidate.amount, candidate.result) {
			t.Fatalf("bad repair passed: %+v", candidate)
		}
	}
	if !reserveSafety(math.MaxInt64, math.MaxInt64, reserveResult{Tag: "Ok", Value: "0"}) {
		t.Fatal("valid exhaustion failed")
	}
}

func TestFirstPairOverflowPrecedesSecondPairBounds(t *testing.T) {
	result := execute(request{ID: "order", Task: "pair_totals", Args: []json.RawMessage{json.RawMessage(`["9223372036854775807","1"]`)}})
	if result.Outcome.Kind != "trap" || result.Outcome.Category != "integer_overflow" {
		t.Fatalf("wrong first failure: %+v", result)
	}
}
