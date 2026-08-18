#include "bridge_output_mapper.h"

#include <algorithm>

namespace tdx_czsc {

void FillZeros(int data_len, float* out_values) {
    if (data_len <= 0 || out_values == nullptr) {
        return;
    }
    std::fill(out_values, out_values + data_len, 0.0f);
}

bool FillFromSnapshot(const AnalysisSnapshotCpp& snapshot, uint32_t func_id, int data_len, float* out_values) {
    if (data_len <= 0 || out_values == nullptr) {
        return false;
    }

    const auto it = snapshot.channels.find(func_id);
    if (it == snapshot.channels.end()) {
        FillZeros(data_len, out_values);
        return false;
    }

    const auto& values = it->second;
    const int count = std::min<int>(data_len, static_cast<int>(values.size()));
    std::copy_n(values.begin(), count, out_values);
    if (count < data_len) {
        std::fill(out_values + count, out_values + data_len, 0.0f);
    }
    return true;
}

}  // namespace tdx_czsc
