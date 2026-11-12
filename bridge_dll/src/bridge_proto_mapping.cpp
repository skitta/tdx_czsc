#include "bridge_proto_mapping.h"

#include <sstream>
#include <utility>
#include <vector>

namespace tdx_czsc {
namespace {

void FillRepeatedFloat(const std::vector<float>& src, google::protobuf::RepeatedField<float>* dst) {
    if (dst == nullptr) {
        return;
    }
    dst->Reserve(static_cast<int>(src.size()));
    for (float value : src) {
        dst->Add(value);
    }
}

void FillRepeatedUInt32(const std::vector<std::uint32_t>& src,
                        google::protobuf::RepeatedField<std::uint32_t>* dst) {
    if (dst == nullptr) {
        return;
    }
    dst->Reserve(static_cast<int>(src.size()));
    for (std::uint32_t value : src) {
        dst->Add(value);
    }
}

void CopyChannelsFromProto(const tdx::czsc::v1::AnalysisSnapshot& proto_snapshot,
                           AnalysisSnapshotCpp* out_snapshot) {
    out_snapshot->channels.clear();
    for (const auto& channel : proto_snapshot.channels()) {
        std::vector<float> values;
        values.reserve(static_cast<std::size_t>(channel.values_size()));
        for (float value : channel.values()) {
            values.push_back(value);
        }
        out_snapshot->channels.emplace(channel.func_id(), std::move(values));
    }
}

}  // namespace

void BuildAnalyzeEnvelope(const AnalyzeRequestCpp& request, tdx::czsc::v1::RequestEnvelope* envelope) {
    envelope->set_protocol_version(request.protocol_version);
    envelope->set_request_id(request.request_id);

    auto* analyze = envelope->mutable_analyze();
    analyze->set_func_group(request.func_group);
    FillRepeatedUInt32(request.requested_func_ids, analyze->mutable_requested_func_ids());
    analyze->set_freq_hint(request.freq_hint);
    analyze->set_data_len(request.data_len);
    analyze->set_input_hash(request.input_hash);
    analyze->set_prefer_cached(request.prefer_cached);

    auto* series = analyze->mutable_series();
    FillRepeatedFloat(request.high, series->mutable_high());
    FillRepeatedFloat(request.low, series->mutable_low());
    FillRepeatedFloat(request.close, series->mutable_close());
    FillRepeatedFloat(request.vol, series->mutable_vol());

    auto* options = analyze->mutable_options();
    options->set_max_bi_num(request.options.max_bi_num);
    options->set_min_bi_len(request.options.min_bi_len);
    options->set_analysis_window(request.options.analysis_window);
}

bool ParseAnalyzeResponse(const tdx::czsc::v1::ResponseEnvelope& envelope,
                          AnalysisSnapshotCpp* out_snapshot,
                          std::string* out_error) {
    if (envelope.status() != tdx::czsc::v1::STATUS_OK) {
        if (out_error != nullptr) {
            std::ostringstream oss;
            oss << "service returned status=" << envelope.status()
                << " error=" << envelope.error_message();
            *out_error = oss.str();
        }
        return false;
    }

    if (!envelope.has_analyze()) {
        if (out_error != nullptr) {
            *out_error = "response body does not contain analyze";
        }
        return false;
    }

    const auto& analyze = envelope.analyze();
    if (!analyze.has_snapshot()) {
        if (out_error != nullptr) {
            *out_error = "analyze response does not contain snapshot";
        }
        return false;
    }

    const auto& snapshot = analyze.snapshot();
    if (snapshot.has_meta()) {
        out_snapshot->input_hash = snapshot.meta().input_hash();
        out_snapshot->data_len = snapshot.meta().data_len();
    } else {
        out_snapshot->input_hash = 0;
        out_snapshot->data_len = 0;
    }

    CopyChannelsFromProto(snapshot, out_snapshot);
    return true;
}

}  // namespace tdx_czsc
