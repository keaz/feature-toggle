/**
 * Common type definitions for API responses
 */

export interface PaginatedResponse<T> {
    items: T[];
    meta: {
        total: number;
        page?: number;
        limit?: number;
    };
}

export interface BaseEntity {
    id: string;
    createdAt: string;
    updatedAt?: string;
}

export interface Environment extends BaseEntity {
    name: string;
    active: boolean;
    environmentType?: string;
}

export interface Context extends BaseEntity {
    key: string;
    entries: string[];
}

export interface Team extends BaseEntity {
    name: string;
    description?: string;
}

export interface Role extends BaseEntity {
    name: string;
    permissions?: string[];
}

export interface User extends BaseEntity {
    username: string;
    email: string;
    firstName?: string;
    lastName?: string;
}

export interface Client extends BaseEntity {
    name: string;
    description?: string;
    clientType: 'WEB' | 'BACKEND';
    environmentId?: string;
    webOrigins?: string[];
    secret?: string;
    secretKey?: string;
}

export interface FeatureStage {
    environmentId: string;
    enabled: boolean;
    rolloutPercentage: number;
}

export interface Feature extends BaseEntity {
    name: string;
    description?: string;
    featureType: 'SIMPLE' | 'CONTEXTUAL';
    defaultValue: boolean;
    stages?: FeatureStage[];
}

export interface PipelineStage {
    name: string;
    order: number;
    environmentId?: string;
}

export interface Pipeline extends BaseEntity {
    name: string;
    description?: string;
    stages: PipelineStage[];
}

export interface Criterion extends BaseEntity {
    stageId: string;
    priority: number;
    groups: CriterionGroup[];
    variantSelectionMode: string;
    enabled: boolean;
}

export interface CriterionGroup {
    logicOperator: 'AND' | 'OR';
    conditions: CriterionCondition[];
}

export interface CriterionCondition {
    contextKey: string;
    operator: string;
    value: string;
}

export interface ApprovalPolicy extends BaseEntity {
    name: string;
    requiredApprovers: number;
    approverRoles: string[];
    appliesTo: string;
    environmentIds?: string[];
}

export interface ApprovalRequest extends BaseEntity {
    status: string;
    featureId?: string;
    requesterId?: string;
    comment?: string;
}

export interface LoginResponse {
    token: string;
    userId?: string;
    expiresAt?: string;
}

export interface AuthStatus {
    id: string;
    username: string;
    email?: string;
}

export interface ApiError {
    error: string;
    message?: string;
}
