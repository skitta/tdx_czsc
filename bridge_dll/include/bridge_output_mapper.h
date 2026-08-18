#pragma once

#include <cstdint>

#include "bridge_service_protocol.h"

namespace tdx_czsc {

void FillZeros(int data_len, float* out_values);
bool FillFromSnapshot(const AnalysisSnapshotCpp& snapshot, uint32_t func_id, int data_len, float* out_values);

}  // namespace tdx_czsc
