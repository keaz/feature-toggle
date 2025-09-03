-- Create roles table with predefined roles
CREATE TABLE roles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    name VARCHAR(50) NOT NULL UNIQUE,
    description TEXT NOT NULL,
    created_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP
);

-- Create user_roles junction table for many-to-many relationship
CREATE TABLE user_roles (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id UUID NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    assigned_at TIMESTAMP WITH TIME ZONE NOT NULL DEFAULT CURRENT_TIMESTAMP,
    assigned_by UUID REFERENCES users(id) ON DELETE SET NULL,
    UNIQUE(user_id, role_id)
);

-- Insert predefined roles
INSERT INTO roles (id, name, description) VALUES 
    ('00000000-0000-0000-0000-000000000001', 'Approver', 'Can approve deployment requests and stage changes'),
    ('00000000-0000-0000-0000-000000000002', 'Requester', 'Can request deployment and stage changes'),
    ('00000000-0000-0000-0000-000000000003', 'Team Admin', 'Can manage team settings and members');

-- Create indexes for efficient lookups
CREATE INDEX idx_user_roles_user_id ON user_roles(user_id);
CREATE INDEX idx_user_roles_role_id ON user_roles(role_id);
CREATE INDEX idx_roles_name ON roles(name);
