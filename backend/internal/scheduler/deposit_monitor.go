package scheduler

import (
	"context"
	"encoding/json"
	"fmt"
	"math/big"
	"net/http"
	"strings"
	"time"

	"github.com/rs/zerolog/log"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/config"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/model"
	"github.com/wisent-ai/compute.wisent.com/backend/internal/repository"
)

type DepositMonitor struct {
	cryptoRepo  *repository.CryptoRepo
	billingRepo *repository.BillingRepo
	cfg         *config.Config
}

func NewDepositMonitor(cr *repository.CryptoRepo, br *repository.BillingRepo, cfg *config.Config) *DepositMonitor {
	return &DepositMonitor{cryptoRepo: cr, billingRepo: br, cfg: cfg}
}

func (dm *DepositMonitor) Start(ctx context.Context) {
	if dm.cfg.WSTTokenAddress == "" || dm.cfg.BaseRPCURL == "" {
		log.Warn().Msg("deposit monitor: WST token or RPC not configured, skipping")
		return
	}

	ticker := time.NewTicker(15 * time.Second)
	defer ticker.Stop()

	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			dm.scan(ctx)
			dm.creditConfirmed(ctx)
		}
	}
}

func (dm *DepositMonitor) scan(ctx context.Context) {
	addresses, err := dm.cryptoRepo.GetAllDepositAddresses(ctx, "base")
	if err != nil || len(addresses) == 0 {
		return
	}

	addrToUser := make(map[string]model.CryptoDepositAddress)
	var topics []string
	for _, a := range addresses {
		addrToUser[strings.ToLower(a.Address)] = a
		padded := "0x000000000000000000000000" + strings.TrimPrefix(strings.ToLower(a.Address), "0x")
		topics = append(topics, padded)
	}

	currentBlock, err := dm.getCurrentBlock()
	if err != nil {
		log.Error().Err(err).Msg("deposit monitor: failed to get current block")
		return
	}

	fromBlock := currentBlock - 100
	if fromBlock < 0 {
		fromBlock = 0
	}

	transferTopic := "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"

	logsPayload := map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  "eth_getLogs",
		"params": []interface{}{
			map[string]interface{}{
				"fromBlock": fmt.Sprintf("0x%x", fromBlock),
				"toBlock":   "latest",
				"address":   dm.cfg.WSTTokenAddress,
				"topics":    []interface{}{transferTopic, nil, topics},
			},
		},
		"id": 1,
	}

	var result struct {
		Result []struct {
			TransactionHash string   `json:"transactionHash"`
			BlockNumber     string   `json:"blockNumber"`
			Topics          []string `json:"topics"`
			Data            string   `json:"data"`
		} `json:"result"`
	}

	if err := dm.rpcCall(logsPayload, &result); err != nil {
		log.Error().Err(err).Msg("deposit monitor: getLogs failed")
		return
	}

	for _, logEntry := range result.Result {
		if len(logEntry.Topics) < 3 {
			continue
		}

		toAddr := "0x" + logEntry.Topics[2][26:]
		depositAddr, ok := addrToUser[strings.ToLower(toAddr)]
		if !ok {
			continue
		}

		amountBig := new(big.Int)
		amountBig.SetString(strings.TrimPrefix(logEntry.Data, "0x"), 16)

		blockNum := new(big.Int)
		blockNum.SetString(strings.TrimPrefix(logEntry.BlockNumber, "0x"), 16)

		confs := currentBlock - blockNum.Int64()

		weiPerCent := new(big.Int)
		weiPerCent.Exp(big.NewInt(10), big.NewInt(16), nil)
		usdCents := new(big.Int).Div(amountBig, weiPerCent).Int64()

		status := model.CryptoTxPending
		if confs >= int64(dm.cfg.DepositConfirmations) {
			status = model.CryptoTxConfirmed
		}

		deposit := model.CryptoDeposit{
			UserID:         depositAddr.UserID,
			Chain:          "base",
			TxHash:         logEntry.TransactionHash,
			TokenAddress:   dm.cfg.WSTTokenAddress,
			TokenSymbol:    "WST",
			AmountWei:      amountBig.String(),
			AmountUSDCents: usdCents,
			Status:         status,
			BlockNumber:    blockNum.Int64(),
			Confirmations:  int(confs),
		}

		_, err := dm.cryptoRepo.InsertDeposit(ctx, deposit)
		if err != nil {
			log.Error().Err(err).Str("tx", logEntry.TransactionHash).Msg("deposit insert failed")
		}
	}
}

func (dm *DepositMonitor) creditConfirmed(ctx context.Context) {
	deposits, err := dm.cryptoRepo.ListPendingDeposits(ctx, "base")
	if err != nil {
		return
	}

	for _, d := range deposits {
		if d.Status != model.CryptoTxConfirmed {
			continue
		}

		desc := fmt.Sprintf("WST deposit: %s", d.TxHash[:16]+"...")
		_, err := dm.billingRepo.AddCredits(ctx, d.UserID, d.AmountUSDCents, model.TransactionPurchase, desc, nil)
		if err != nil {
			log.Error().Err(err).Str("deposit", d.ID.String()).Msg("credit failed")
			continue
		}

		_ = dm.cryptoRepo.MarkDepositCredited(ctx, d.ID)
		log.Info().Str("user", d.UserID.String()).Int64("cents", d.AmountUSDCents).Msg("deposit credited")
	}
}

func (dm *DepositMonitor) getCurrentBlock() (int64, error) {
	var result struct {
		Result string `json:"result"`
	}
	payload := map[string]interface{}{"jsonrpc": "2.0", "method": "eth_blockNumber", "params": []interface{}{}, "id": 1}
	if err := dm.rpcCall(payload, &result); err != nil {
		return 0, err
	}
	blockNum := new(big.Int)
	blockNum.SetString(strings.TrimPrefix(result.Result, "0x"), 16)
	return blockNum.Int64(), nil
}

func (dm *DepositMonitor) rpcCall(payload interface{}, result interface{}) error {
	body, _ := json.Marshal(payload)
	resp, err := http.Post(dm.cfg.BaseRPCURL, "application/json", strings.NewReader(string(body)))
	if err != nil {
		return err
	}
	defer resp.Body.Close()
	return json.NewDecoder(resp.Body).Decode(result)
}
